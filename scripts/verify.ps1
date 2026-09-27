<#
.SYNOPSIS
  Start the shell binary, talk to it over a real socket, then ask it to stop.

.DESCRIPTION
  This is the acceptance script for one round of shell work. It uses only the
  shipped artifact and a real socket, and it checks the things that are easy to
  get subtly wrong:

    * the URL on the console is the one that works (the port parses, and the page
      answers without anything else in the URL)
    * the page and every asset it references answer without a query string, and a
      second request for `/` - a reload - answers the same way
    * a foreign Origin and a rebound Host are refused in BOTH shapes
      (403 page / 403 JSON)
    * the page, its assets and the API all answer
    * a Range request gets 206 and exactly the bytes asked for
    * the console says it is bound to loopback, and never to 0.0.0.0
    * /api/shutdown answers, the process exits 0, and the port is released
    * a closed stdout pipe does not kill the process (logging must never panic)

  Exit code 0 means every check passed. Non-zero prints the failures with the
  response that produced them.

.NOTES
  Written against System.Net.Sockets rather than System.Net.Http: this machine's
  PowerShell runs in ConstrainedLanguage mode, where the HttpClient types cannot
  be resolved. Raw sockets also make the request text explicit, which is what a
  reader of a failing check actually wants to see.

  ASCII only, like every .ps1 in this tree: PowerShell 5.1 reads BOM-less files
  as ANSI.

.EXAMPLE
  powershell -File scripts\verify.ps1
  powershell -File scripts\verify.ps1 -Configuration release
#>

param(
    [ValidateSet('debug', 'release')]
    [string]$Configuration = 'debug'
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root "target\$Configuration\hongshi.exe"
if (-not (Test-Path $exe)) {
    throw "no binary at $exe - build it first: powershell -File scripts\build.ps1"
}

$failures = New-Object System.Collections.Generic.List[string]
$script:checks = 0

function Assert-That([string]$name, [bool]$condition, [string]$detail) {
    $script:checks++
    if ($condition) {
        Write-Host ("  PASS  {0}" -f $name) -ForegroundColor Green
    }
    else {
        Write-Host ("  FAIL  {0}" -f $name) -ForegroundColor Red
        if ($detail) {
            $shown = $detail
            if ($shown.Length -gt 400) { $shown = $shown.Substring(0, 400) + ' ...' }
            Write-Host ("        {0}" -f $shown.Replace("`r", '').Replace("`n", ' | ')) -ForegroundColor DarkGray
        }
        $script:failures.Add($name)
    }
}

# ---------------------------------------------------------------------------
# raw HTTP over a socket

function Send-RawRequest([int]$port, [string]$raw) {
    # One retry, and only when the connection produced nothing at all. A client
    # that connects within milliseconds of the process starting occasionally has
    # its socket closed under it by PowerShell's own machinery - the Rust probe in
    # examples/first_request_probe.rs does the same thing 60 times out of 60 - so
    # a bare timeout here says more about the harness than about the shell. A
    # response that arrives and is wrong is never retried.
    $attempt = Send-RawOnce $port $raw
    if ($attempt.Status -eq 0 -and [string]::IsNullOrEmpty($attempt.Text)) {
        Start-Sleep -Milliseconds 200
        $attempt = Send-RawOnce $port $raw
    }
    return $attempt
}

function Send-RawOnce([int]$port, [string]$raw) {
    $client = [System.Net.Sockets.TcpClient]::new()
    try {
        $client.Connect('127.0.0.1', $port)
        $client.ReceiveTimeout = 8000
        $stream = $client.GetStream()
        $bytes = [System.Text.Encoding]::ASCII.GetBytes($raw)
        $stream.Write($bytes, 0, $bytes.Length)
        $stream.Flush()

        $buffer = New-Object byte[] 65536
        $builder = New-Object System.Text.StringBuilder
        $timedOut = $false
        try {
            while ($true) {
                $read = $stream.Read($buffer, 0, $buffer.Length)
                if ($read -le 0) { break }
                [void]$builder.Append([System.Text.Encoding]::UTF8.GetString($buffer, 0, $read))
            }
        }
        catch {
            $timedOut = $true
        }

        $text = $builder.ToString()
        $status = 0
        $match = [regex]::Match($text, '^HTTP/1\.1 (\d{3})')
        if ($match.Success) { $status = [int]$match.Groups[1].Value }
        $body = ''
        $split = $text.IndexOf("`r`n`r`n")
        if ($split -ge 0) { $body = $text.Substring($split + 4) }

        return [pscustomobject]@{
            Status   = $status
            Text     = $text
            Body     = $body
            TimedOut = $timedOut
        }
    }
    finally {
        $client.Close()
    }
}

function Send-Request([int]$port, [string]$method, [string]$target, [string]$host_, [hashtable]$headers, [string]$body) {
    $lines = New-Object System.Collections.Generic.List[string]
    $lines.Add("$method $target HTTP/1.1")
    $lines.Add("Host: $host_")
    if ($null -ne $body) { $lines.Add("Content-Length: $($body.Length)") }
    foreach ($key in $headers.Keys) { $lines.Add("${key}: $($headers[$key])") }
    $lines.Add('Connection: close')
    $lines.Add('')
    $lines.Add('')
    $raw = ($lines -join "`r`n")
    if ($null -ne $body) { $raw += $body }
    return Send-RawRequest $port $raw
}

# ---------------------------------------------------------------------------
# start the shell

Write-Host ''
Write-Host "starting $exe --no-browser" -ForegroundColor Cyan

$psi = [System.Diagnostics.ProcessStartInfo]::new()
$psi.FileName = $exe
$psi.Arguments = '--no-browser'
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$psi.UseShellExecute = $false
$psi.CreateNoWindow = $true
$process = [System.Diagnostics.Process]::Start($psi)

$lines = New-Object System.Collections.Generic.List[string]
$url = $null
for ($i = 0; $i -lt 100 -and -not $url; $i++) {
    $line = $process.StandardOutput.ReadLine()
    if ($null -eq $line) { break }
    $lines.Add($line) | Out-Null
    if ($line.Trim().StartsWith('URL')) { $url = $line.Trim().Substring(3).Trim() }
}

if (-not $url) {
    try { $process.Kill() } catch { }
    throw "the shell never printed a URL. output was:`n$($lines -join "`n")"
}
Write-Host "console URL: $url" -ForegroundColor Cyan

<#
The banner does not end at the URL line: `bound`, `port` and `UI` follow it, and
they are the lines that explain where the page came from. Draining whatever the
console has already produced (non-blocking, so a quiet shell cannot park the
script) is what makes those two checks about the console rather than about the
loop above.
#>
function Receive-ConsoleLines([int]$milliseconds) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $milliseconds) {
        if ($process.StandardOutput.Peek() -lt 0) { Start-Sleep -Milliseconds 20; continue }
        $line = $process.StandardOutput.ReadLine()
        if ($null -eq $line) { break }
        $lines.Add($line) | Out-Null
        $sw.Restart()
    }
}
Receive-ConsoleLines 500

# Drain the console for the rest of the run, on a background .NET read.
#
# Nothing reads stdout between here and the end of the script, and a pipe holds only
# a few kilobytes: without this, the middle of the console is lost, and the checks
# that read it at the end ("the shell logged the requests it served", "no forged log
# line reached the console") quietly assert against a truncated transcript. This is
# also what filled the pipe enough to block the shell before logging was made
# asynchronous: the harness was the slow reader.
$consoleTask = $process.StandardOutput.ReadToEndAsync()

$match = [regex]::Match($url, '^http://127\.0\.0\.1:(\d+)/$')
Assert-That 'the console prints a plain loopback URL' $match.Success $url
if (-not $match.Success) {
    try { $process.Kill() } catch { }
    throw 'cannot continue without a parsable URL'
}
$port = [int]$match.Groups[1].Value
Assert-That 'the URL carries no query string for a reload to lose' (-not $url.Contains('?')) $url
$authority = "127.0.0.1:$port"

try {
    # -----------------------------------------------------------------------
    Write-Host ''
    Write-Host 'listener' -ForegroundColor Cyan
    $bound = $lines | Where-Object { $_.Trim().StartsWith('bound') } | Select-Object -First 1
    Assert-That 'the console says it bound loopback' ($null -ne $bound -and $bound -match '127\.0\.0\.1') $bound
    Assert-That 'nothing in the console claims 0.0.0.0' (-not ($lines -match '0\.0\.0\.0')) ($lines -join ' | ')
    $ui = $lines | Where-Object { $_.Trim().StartsWith('UI') } | Select-Object -First 1
    Assert-That 'the console names where the UI comes from' ($null -ne $ui) ($lines -join ' | ')

    # -----------------------------------------------------------------------
    Write-Host ''
    Write-Host 'the page needs no ceremony' -ForegroundColor Cyan
    $page = Send-Request $port 'GET' "/" $authority @{} $null
    Assert-That 'the page loads with nothing in the URL' ($page.Status -eq 200) "status $($page.Status)"
    # A reload is the same request. This is the regression: `/` used to be gated
    # behind a `?token=...`, so pressing F5 got a 403 and the whole UI died.
    $reload = Send-Request $port 'GET' "/" $authority @{} $null
    Assert-That 'a reload of the page works' ($reload.Status -eq 200) "status $($reload.Status)"
    Assert-That 'both requests served the same bytes' ($reload.Body -eq $page.Body) 'the two bodies differ'
    Assert-That 'the page is served verbatim, not rewritten in flight' ($page.Body.Contains('href="main.css"')) $page.Body
    # Matched on ASCII on purpose. This file has no BOM (every .ps1 here is
    # ASCII-only for the same reason), so Windows PowerShell 5.1 decodes it as
    # ANSI and Chinese literals inside it do not survive - the assertion then
    # fails against a page that is perfectly fine, which is what happened.
    Assert-That 'the page is the shell UI' ($page.Body.Contains('lang="zh-CN"') -and $page.Body.Contains('class="side"')) $page.Body
    Assert-That 'the page is not cacheable' ($page.Text -match '(?i)cache-control: no-store') $page.Text
    Assert-That 'the page forbids framing' ($page.Text -match "frame-ancestors 'none'") $page.Text

    # -----------------------------------------------------------------------
    # The page's own assets, requested the way a BROWSER requests them.
    #
    # A browser resolves `href="main.css"` against the page URL, so it asks for
    # `/main.css` exactly as written. This block is the one that was missing when
    # the UI shipped unable to load its own stylesheet: every other check used a
    # URL with a token in it, a request no browser ever makes.
    #
    # One detail the first version got wrong, which showed up as a wall of failures
    # that said nothing about the shell: a fragment is resolved by the browser and
    # not sent to the server, so `icons.svg#i-home` must be requested as far as the
    # `#`.
    Write-Host ''
    Write-Host 'the page can load itself' -ForegroundColor Cyan
    $references = [regex]::Matches($page.Body, '(?:href|src)="([^"]+)"') | ForEach-Object { $_.Groups[1].Value }
    $assets = @($references | Where-Object { ($_ -split '[?#]')[0].Split('/')[-1].Contains('.') })
    $routes = @($references | Where-Object { -not ($_ -split '[?#]')[0].Split('/')[-1].Contains('.') })

    Assert-That 'the page references its stylesheets, sprite and scripts' ($assets.Count -ge 5) ($assets -join ', ')
    Assert-That 'the page links its own routes' ($routes.Count -ge 6) ($routes -join ', ')

    foreach ($expected in @('main.css', 'app.css', 'icons.svg', 'app.js', 'pages.js')) {
        $found = @($assets | Where-Object { $_ -like "*$expected*" })
        Assert-That "the page references $expected" ($found.Count -ge 1) ($assets -join ', ')
        if ($found.Count -ge 1) {
            Assert-That "$expected is referenced exactly as the file is named" ($found[0] -notmatch 'token=') $found[0]
        }
    }
    Assert-That 'the sprite reference kept its fragment' (@($assets | Where-Object { $_ -match 'icons\.svg#' }).Count -ge 1) ($assets -join ', ')

    foreach ($asset in $assets) {
        $withoutFragment = ($asset -split '#')[0]
        $target = if ($withoutFragment.StartsWith('/')) { $withoutFragment } else { "/$withoutFragment" }
        $response = Send-Request $port 'GET' $target $authority @{} $null
        Assert-That ("a browser can fetch $target") ($response.Status -eq 200 -and $response.Body.Length -gt 100) "status $($response.Status), $($response.Body.Length) bytes"
    }

    foreach ($route in $routes) {
        Assert-That "the route $route is not rewritten" ($route -notmatch '\?') $route
        $response = Send-Request $port 'GET' "$route" $authority @{} $null
        Assert-That "a browser can reach $route" ($response.Status -eq 200) "status $($response.Status)"
    }

    foreach ($bare in @('/main.css', '/app.js', '/index.html')) {
        $served = Send-Request $port 'GET' $bare $authority @{} $null
        Assert-That ("$bare is served to a plain request") ($served.Status -eq 200) "status $($served.Status)"
    }

    # -----------------------------------------------------------------------
    Write-Host ''
    Write-Host 'assets and API' -ForegroundColor Cyan
    $css = Send-Request $port 'GET' "/main.css" $authority @{} $null
    Assert-That 'main.css is served as text/css' ($css.Status -eq 200 -and $css.Text -match 'content-type: text/css') "status $($css.Status)"
    Assert-That 'main.css carries the design tokens' ($css.Body.Contains('--ground: #313131')) $css.Body

    $js = Send-Request $port 'GET' "/app.js" $authority @{} $null
    Assert-That 'app.js is served' ($js.Status -eq 200 -and $js.Body.Contains('/api/health')) "status $($js.Status)"

    # Every `url()` in the stylesheet is a reference the *browser* resolves, so each
    # one has to be fetchable as written. A miss here is a font or a background that
    # silently never loads, and the browser reports it only as "the face failed".
    $cssUrls = [regex]::Matches($css.Body, 'url\(([^)]+)\)') | ForEach-Object { $_.Groups[1].Value.Trim('"', "'") }
    Assert-That 'the stylesheet has url() references to check' ($cssUrls.Count -ge 1) ($cssUrls -join ', ')
    foreach ($reference in $cssUrls) {
        if ($reference -match '^(https?:|data:|//)') { continue }
        $name = ($reference -split '\?')[0]
        # Fetched exactly as it was written, which is what the browser does.
        $fetched = Send-Request $port 'GET' "/$name" $authority @{} $null
        Assert-That "the stylesheet's $name reference resolves" ($fetched.Status -eq 200) "status $($fetched.Status) for /$name"
    }

    $health = Send-Request $port 'GET' "/api/health" $authority @{} $null
    Assert-That 'health answers JSON' ($health.Status -eq 200 -and $health.Body.StartsWith('{')) $health.Body
    Assert-That 'health reports the kernel block' ($health.Body -match '"kernel":\{.*"running":false') $health.Body
    Assert-That 'health names the UI source' ($health.Body.Contains('web_source')) $health.Body
    Assert-That 'health reports its own pid' ($health.Body.Contains("`"pid`":$($process.Id)")) $health.Body

    $echo = Send-Request $port 'POST' "/api/echo" $authority @{} 'a=1&b=hello%20world'
    Assert-That 'a posted body is read and decoded' ($echo.Status -eq 200 -and $echo.Body -eq '{"a":"1","b":"hello world"}') "$($echo.Status) $($echo.Body)"

    # -----------------------------------------------------------------------
    Write-Host ''
    Write-Host 'refusals' -ForegroundColor Cyan
    $missing = Send-Request $port 'GET' "/nope.html" $authority @{} $null
    Assert-That 'an unknown file is 404' ($missing.Status -eq 404) "status $($missing.Status)"

    $unknownApi = Send-Request $port 'GET' "/api/nope" $authority @{} $null
    # 404, not a 200 with an error body: a caller checking `response.ok` must not
    # read "no such endpoint" as success.
    Assert-That 'an unknown endpoint is a real 404 in JSON' ($unknownApi.Status -eq 404 -and $unknownApi.Body.Contains('no such endpoint')) "status $($unknownApi.Status): $($unknownApi.Body)"

    # The gate is where a request came from, not what it carries. A page on some
    # other origin is refused in both shapes, for every path, whether or not the
    # path exists - a 404 for a stranger would be a free "does this exist" oracle.
    foreach ($path in @('/', '/main.css', '/api/health', '/api/nope')) {
        $foreign = Send-Request $port 'GET' $path $authority @{ 'Origin' = 'http://evil.example' } $null
        Assert-That "a foreign Origin is refused for $path" ($foreign.Status -eq 403) "status $($foreign.Status)"
    }
    $foreignApi = Send-Request $port 'GET' '/api/health' $authority @{ 'Origin' = 'http://evil.example' } $null
    Assert-That 'the API refusal is JSON, not a page' ($foreignApi.Text -match 'application/json') $foreignApi.Text
    Assert-That 'the API refusal is a real 403, not a 200 with an error body' ($foreignApi.Status -eq 403 -and $foreignApi.Body -match 'refused') "$($foreignApi.Status): $($foreignApi.Body)"

    $rebound = Send-Request $port 'GET' "/api/health" 'evil.example' @{} $null
    Assert-That 'a rebound Host header is refused' ($rebound.Status -eq 403) "status $($rebound.Status)"

    # A browser navigation carries neither header, and must not be caught by this.
    $ownOrigin = Send-Request $port 'GET' '/api/health' $authority @{ 'Origin' = "http://$authority" } $null
    Assert-That 'an Origin matching our own authority is allowed' ($ownOrigin.Status -eq 200) "status $($ownOrigin.Status)"

    $malformed = Send-RawRequest $port "GET`r`n`r`n"
    Assert-That 'a malformed request line gets a 400, not a dropped connection' ($malformed.Status -eq 400) "status $($malformed.Status), timed out: $($malformed.TimedOut)"

    # -----------------------------------------------------------------------
    Write-Host ''
    Write-Host 'range support' -ForegroundColor Cyan
    $range = Send-Request $port 'GET' "/app.js" $authority @{ 'Range' = 'bytes=0-9' } $null
    Assert-That 'a Range request is 206' ($range.Status -eq 206) "status $($range.Status)"
    Assert-That 'a Range request returns exactly ten bytes' ($range.Body.Length -eq 10) "length $($range.Body.Length)"
    Assert-That 'the response carries Content-Range' ($range.Text -match 'content-range: bytes 0-9/\d+') $range.Text

    $unsatisfiable = Send-Request $port 'GET' "/app.js" $authority @{ 'Range' = 'bytes=999999-' } $null
    Assert-That 'an unsatisfiable Range is 416' ($unsatisfiable.Status -eq 416) "status $($unsatisfiable.Status)"

    # RFC 9110 section 14.2 / section 14.5.1: a range unit or set the server will not honour is
    # IGNORED, not refused. 416 means "those bytes are not there", which is untrue
    # of a well-formed multi-range request.
    $multi = Send-Request $port 'GET' "/app.js" $authority @{ 'Range' = 'bytes=0-10,20-30' } $null
    Assert-That 'a multi-range request is answered in full, not refused' ($multi.Status -eq 200) "status $($multi.Status)"
    $wrongUnit = Send-Request $port 'GET' "/app.js" $authority @{ 'Range' = 'items=0-1' } $null
    Assert-That 'an unknown range unit is ignored, not refused' ($wrongUnit.Status -eq 200) "status $($wrongUnit.Status)"

    # -----------------------------------------------------------------------
    Write-Host ''
    Write-Host 'connection limit' -ForegroundColor Cyan
    # A local process must not be able to pin unbounded threads by opening sockets
    # and saying nothing. The cap is 32; the connection past it is told 503 rather
    # than being left to hang or refused at the TCP layer.
    #
    # Each held connection sends a partial request line, so it is inside a request
    # holding its slot rather than idle. The check is made immediately after, well
    # inside the request deadline, so slots cannot have been released underneath it.
    $held = New-Object System.Collections.Generic.List[object]
    for ($i = 0; $i -lt 32; $i++) {
        $socket = [System.Net.Sockets.TcpClient]::new()
        try {
            $socket.Connect('127.0.0.1', $port)
            $partial = [System.Text.Encoding]::ASCII.GetBytes("GET /slow$i HTTP/1.1`r`n")
            $socket.GetStream().Write($partial, 0, $partial.Length)
            $socket.GetStream().Flush()
            $held.Add($socket)
        }
        catch { break }
    }
    Assert-That 'the shell accepts connections up to its cap' ($held.Count -eq 32) "opened $($held.Count)"

    $over = [System.Net.Sockets.TcpClient]::new()
    try {
        $over.Connect('127.0.0.1', $port)
        $over.ReceiveTimeout = 4000
        # A path the server serves, so a 503 cannot be mistaken for a 404 or a 403:
        # every other check in this block would pass against a server with no cap.
        $request = [System.Text.Encoding]::ASCII.GetBytes("GET / HTTP/1.1`r`nHost: $authority`r`nConnection: close`r`n`r`n")
        $over.GetStream().Write($request, 0, $request.Length)
        $over.GetStream().Flush()
        $buffer = New-Object byte[] 8192
        $read = $over.GetStream().Read($buffer, 0, $buffer.Length)
        $text = [System.Text.Encoding]::UTF8.GetString($buffer, 0, $read)
        Assert-That 'the connection past the cap is told 503' ($text -match 'HTTP/1.1 503') $text
    }
    catch {
        Assert-That 'the connection past the cap is told 503' $false $_.Exception.Message
    }
    finally {
        try { $over.Close() } catch { }
    }

    foreach ($socket in $held) { try { $socket.Close() } catch { } }
    Start-Sleep -Milliseconds 500
    $afterCap = Send-Request $port 'GET' "/api/health" $authority @{} $null
    Assert-That 'the shell still serves once those connections close' ($afterCap.Status -eq 200) "status $($afterCap.Status)"

    # -----------------------------------------------------------------------
    Write-Host ''
    Write-Host 'log forgery' -ForegroundColor Cyan
    # Refusals are logged too, so a caller that is turned away still reaches the
    # log. The console must not be forgeable from one.
    $null = Send-Request $port 'GET' "/x%0aINFO%20forged%20URL%20http://evil/%20status=200" $authority @{} $null
    $null = Send-Request $port 'GET' "/y%1b%5b31mred" $authority @{} $null
    Start-Sleep -Milliseconds 200

    # -----------------------------------------------------------------------
    Write-Host ''
    Write-Host 'the interface' -ForegroundColor Cyan
    # Every route the sidebar links to is the same document, so a reload or a
    # bookmark lands somewhere sensible.
    foreach ($route in @('/', '/connect', '/settings', '/cloud', '/hosting', '/together')) {
        $r = Send-Request $port 'GET' "$route" $authority @{} $null
        # ASCII markers again: see the note above.
        $isShell = $r.Status -eq 200 -and $r.Body.Contains('class="side"') -and $r.Body.Contains('id="page"')
        Assert-That ("$route serves the shell") $isShell "status $($r.Status)"
    }
    # A file that is not there stays an honest 404 instead of rendering as a page.
    $dotted = Send-Request $port 'GET' "/missing.css" $authority @{} $null
    Assert-That 'a missing asset is a 404, not the shell' ($dotted.Status -eq 404) "status $($dotted.Status)"

    # The sprite is referenced by every icon and must keep its fragment intact, or
    # the whole sidebar renders as empty boxes.
    $sprite = Send-Request $port 'GET' "/icons.svg" $authority @{} $null
    Assert-That 'the icon sprite is served' ($sprite.Status -eq 200 -and $sprite.Body.Contains('i-home')) "status $($sprite.Status)"
    Assert-That 'the sprite reference kept its fragment' ($page.Body -match 'icons\.svg#i-') 'the page markup'

    # The three services that are not built yet still answer, and answer honestly.
    $cloud = Send-Request $port 'GET' "/api/settings" $authority @{} $null
    Assert-That 'settings reports the fields the page edits' ($cloud.Status -eq 200 -and $cloud.Body.Contains('api_base') -and $cloud.Body.Contains('default_game_port')) $cloud.Body

    # -----------------------------------------------------------------------
    # Minecraft 资讯. The shell fetches Mojang's launcher feed and trims it; the page
    # never talks to Mojang itself. This is the one block that needs the internet, so
    # a feed that does not answer is skipped rather than failed - what is asserted is
    # the shape when it does answer, never that the network is up.
    Write-Host ''
    Write-Host 'the Minecraft news' -ForegroundColor Cyan
    $news = Send-Request $port 'GET' '/api/daily-news' $authority @{} $null
    Assert-That 'the news endpoint answers JSON' ($news.Status -eq 200 -and $news.Body.StartsWith('{')) "status $($news.Status): $($news.Body)"

    $feed = $null
    try { $feed = $news.Body | ConvertFrom-Json } catch { }
    Assert-That 'the news body parses' ($null -ne $feed) $news.Body

    if ($null -ne $feed -and $feed.state -eq 'ok') {
        $count = @($feed.data).Count
        # The upstream file carries a hundred entries; the page is given a handful.
        Assert-That 'the feed is trimmed to a handful' ($count -ge 1 -and $count -le 5) "count $count"
        Assert-That 'the source names the publisher, not the service address' ($feed.source -like '*mojang*') $feed.source

        $untitled = @($feed.data | Where-Object { -not $_.title })
        Assert-That 'every item has a title' ($untitled.Count -eq 0) $news.Body

        # The page is served from loopback, so a relative picture would be looked for
        # on the client. The shell completes them against the publisher.
        $relative = @($feed.data | Where-Object { $_.image -and $_.image -notmatch '^https?://' })
        Assert-That 'every picture URL is absolute' ($relative.Count -eq 0) $news.Body

        # ...which is only useful if the policy lets the page load them.
        Assert-That 'the policy allows that origin for pictures' `
            ($page.Text -match "img-src[^;]*launchercontent\.mojang\.com") 'the page CSP'

        # A second look must come out of the cache, not re-read 64 KB.
        $again = Send-Request $port 'GET' '/api/daily-news' $authority @{} $null
        Assert-That 'a second look comes from the cache' (($again.Body | ConvertFrom-Json).origin -eq 'cache') $again.Body
    }
    else {
        Write-Host '  SKIP  the feed did not answer; the shape checks need the internet' -ForegroundColor DarkYellow
    }

    # The kernel: the client either has one and can start it, or has none and says
    # exactly what is missing. What it must never do is answer a start request with a
    # tunnel it did not create.
    $kernel = Send-Request $port 'GET' "/api/kernel" $authority @{} $null
    Assert-That 'the kernel endpoint reports this build platform' ($kernel.Status -eq 200 -and $kernel.Body -match '"platform":"(windows|linux|macos)"' -and $kernel.Body -match '"arch":"(amd64|arm64)"') $kernel.Body
    Assert-That 'it names the file a download would fetch' ($kernel.Body -match '"expected_file":"hongshic-[a-z]+-(amd64|arm64)(\.exe)?"') $kernel.Body
    Assert-That 'it names the directory it looks in' ($kernel.Body -match '"core_dir":"') $kernel.Body
    Assert-That 'the kernel block is in health too' ((Send-Request $port 'GET' "/api/health" $authority @{} $null).Body -match '"kernel":\{') 'health body'

    $log = Send-Request $port 'GET' "/api/kernel/log?since=0" $authority @{} $null
    Assert-That 'the kernel log is pollable from a cursor' ($log.Status -eq 200 -and $log.Body -match '"seq":\d+' -and $log.Body.Contains('"lines":[')) $log.Body
    Assert-That 'the log poll carries the kernel state with it' ($log.Body -match '"kernel":\{') $log.Body

    $tunnel = Send-Request $port 'POST' "/api/tunnel/start" $authority @{} '{}'
    Assert-That 'starting with no relay chosen is a 400 with a reason' ($tunnel.Status -eq 400 -and $tunnel.Body.Contains('bad_request')) "status $($tunnel.Status): $($tunnel.Body)"

    $noKernel = Send-Request $port 'POST' "/api/tunnel/start" $authority @{} '{"relay":"relay.invalid","game_port":25565}'
    $hasKernel = $kernel.Body -match '"found":true'
    if ($hasKernel) {
        Assert-That 'a found kernel is started and reports running' ($noKernel.Status -eq 200 -and $noKernel.Body.Contains('"running":true')) "status $($noKernel.Status): $($noKernel.Body)"
        Assert-That 'no address is claimed before the kernel prints one' ($noKernel.Body.Contains('"endpoint":null')) $noKernel.Body
        # Stop it again. 409 is the honest answer when the relay refused the tunnel and
        # the child has already exited between the two requests; the point of the check
        # is that the client is empty-handed afterwards either way.
        $stop = Send-Request $port 'POST' "/api/tunnel/stop" $authority @{} '{}'
        Assert-That 'stopping answers 200, or 409 once the kernel has already exited' ($stop.Status -eq 200 -or $stop.Status -eq 409) "status $($stop.Status): $($stop.Body)"
        $after = Send-Request $port 'GET' "/api/tunnel/status" $authority @{} $null
        Assert-That 'nothing is left running after the stop' ($after.Body.Contains('"running":false')) $after.Body
    }
    else {
        Assert-That 'a missing kernel is a 424 with the reason, never a fake tunnel' ($noKernel.Status -eq 424 -and $noKernel.Body.Contains('no_kernel') -and $noKernel.Body.Contains('hongshic')) "status $($noKernel.Status): $($noKernel.Body)"
        Assert-That 'nothing was claimed to be running' (-not $noKernel.Body.Contains('"running":true')) $noKernel.Body
    }

    # `status` is a question, and "nothing is running" is a complete answer to it:
    # a 200 with running:false, not an error.
    $status = Send-Request $port 'GET' "/api/tunnel/status" $authority @{} $null
    Assert-That 'tunnel status answers 200 with running:false' ($status.Status -eq 200 -and $status.Body.Contains('"running":false')) "status $($status.Status): $($status.Body)"

    # The probe reports how the number was obtained, because "12 ms by ping" and
    # "12 ms to the control port" are different claims.
    $probe = Send-Request $port 'POST' "/api/probe" $authority @{} '{"hosts":["host.invalid"]}'
    Assert-That 'probing an unreachable host says so' ($probe.Status -eq 200 -and $probe.Body.Contains('"state":"dead"')) $probe.Body
    Assert-That 'the probe names its method' ($probe.Body -match '"method":"(ping|tcp|none)"') $probe.Body

    # A node that *can* carry a tunnel: a listener on the control port. Asserting
    # this against a bare 127.0.0.1 would be asserting that ICMP is available in
    # whatever environment the script runs in, which is not this script's business.
    Write-Host ''
    Write-Host 'latency probe against a live control port' -ForegroundColor Cyan
    $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 8080)
    $probed = $null
    try {
        $listener.Start()
        $accept = $listener.BeginAcceptTcpClient($null, $null)
        $local = Send-Request $port 'POST' "/api/probe" $authority @{} '{"hosts":["127.0.0.1"]}'
        if ($accept.AsyncWaitHandle.WaitOne(2000)) {
            $client = $listener.EndAcceptTcpClient($accept)
            $client.Close()
        }
        $probed = $local
    }
    catch {
        $probed = $null
    }
    finally {
        try { $listener.Stop() } catch { }
    }

    if ($probed) {
        Assert-That 'a listening control port is measured, not refused' ($probed.Body -match '"state":"(ok|slow)"' -and $probed.Body -match '"method":"tcp"') $probed.Body
    }
    else {
        Assert-That 'the probe answered for a live control port' $false 'the probe request did not complete'
    }

    # -----------------------------------------------------------------------
    # An idle socket is not a request.
    #
    # A browser opens several speculative connections per page load and leaves them
    # idle. Each one used to sit until the read timeout and then be answered and
    # logged as a 408 with no method and no path - around a hundred such lines in one
    # ten-minute session, which buries the events the console exists to show. An idle
    # socket is closed in silence now, while a client that began a request and then
    # stalled still gets its 408 and still appears in the log.
    Write-Host ''
    Write-Host 'idle and stalled connections' -ForegroundColor Cyan

    $idle = [System.Net.Sockets.TcpClient]::new()
    $stalled = [System.Net.Sockets.TcpClient]::new()
    try {
        $idle.Connect('127.0.0.1', $port)
        $stalled.Connect('127.0.0.1', $port)

        # A complete request line and a Host header, then silence: the client has
        # committed to a request and stalled inside its headers.
        $stallBytes = [Text.Encoding]::ASCII.GetBytes("GET /api/health HTTP/1.1`r`nHost: 127.0.0.1:$port`r`n")
        $stallStream = $stalled.GetStream()
        $stallStream.Write($stallBytes, 0, $stallBytes.Length)
        $stallStream.Flush()

        # Longer than the client's 5s read timeout, so both outcomes have settled.
        Start-Sleep -Seconds 7

        $buf = New-Object byte[] 512
        $idleStream = $idle.GetStream()
        $idleStream.ReadTimeout = 1500
        $idleRead = -1
        try { $idleRead = $idleStream.Read($buf, 0, $buf.Length) }
        catch { $idleRead = -2 }   # timed out: still open, still nothing sent
        Assert-That 'an idle connection is sent nothing at all' ($idleRead -le 0) "Read returned $idleRead"

        $stallStream.ReadTimeout = 3000
        $stallText = ''
        try {
            $read = $stallStream.Read($buf, 0, $buf.Length)
            if ($read -gt 0) { $stallText = [Text.Encoding]::ASCII.GetString($buf, 0, $read) }
        }
        catch { $stallText = '' }
        Assert-That 'a stalled request still gets a 408' ($stallText -match '^HTTP/1\.1 408') $stallText
    }
    finally {
        try { $idle.Close() } catch { }
        try { $stalled.Close() } catch { }
    }

    # -----------------------------------------------------------------------
    Write-Host ''
    Write-Host 'shutdown' -ForegroundColor Cyan
    $quit = Send-Request $port 'POST' "/api/shutdown" $authority @{} ''
    Assert-That 'the quit endpoint answers 200' ($quit.Status -eq 200) "status $($quit.Status): $($quit.Body)"
    Assert-That 'the quit endpoint says what it is doing' ($quit.Body.Contains('shutting down')) $quit.Body
}
finally {
    # nothing to dispose: each request opens and closes its own socket
}

$exited = $process.WaitForExit(10000)
Assert-That 'the process exits after /api/shutdown' $exited 'still running after 10s'
if ($exited) {
    Assert-That 'an orderly shutdown exits 0' ($process.ExitCode -eq 0) "exit code $($process.ExitCode)"
}
else {
    try { $process.Kill() } catch { }
}

$stdout = $lines -join "`n"
# The rest of the console, collected by the background read started above.
$stdout += "`n" + $consoleTask.GetAwaiter().GetResult()
$stderr = $process.StandardError.ReadToEnd()

Assert-That 'nothing was written to stderr' ([string]::IsNullOrWhiteSpace($stderr)) $stderr
Assert-That 'the shell logged the requests it served' ($stdout -match 'INFO request') ($stdout | Select-Object -Last 1)
Assert-That 'no forged log line reached the console' (-not ($stdout -split "`r?`n" | Where-Object { $_ -match '^INFO forged' })) $stdout
Assert-That 'no raw escape byte reached the console' (-not $stdout.Contains([char]27)) $stdout
Assert-That 'the forged path stayed on the request line' ($stdout -match 'path=/x\?INFO forged') $stdout

# The other half of the idle-socket check: not answering an idle socket is only
# useful if it also stays out of the console.
#
# A connection whose request never parsed is logged with no method and no path, so
# the stalled request above and an idle socket would look identical in the log. The
# check is therefore on the count: exactly one such line, from the stalled request,
# and none from the idle socket. Before the fix the idle socket contributed one line
# per connection - which is what buried the console under a hundred lines that all
# said nothing had happened.
$unparsed408 = @($stdout -split "`r?`n" | Where-Object { $_ -match 'status=408' })
Assert-That 'only the stalled request was logged at 408, not the idle socket' `
    ($unparsed408.Count -eq 1) ("{0} line(s): {1}" -f $unparsed408.Count, ($unparsed408 -join ' | '))

$portFree = $false
$probe = [System.Net.Sockets.TcpClient]::new()
try {
    $connected = $probe.ConnectAsync('127.0.0.1', $port).Wait(1500)
    $portFree = -not $connected
}
catch {
    $portFree = $true
}
finally {
    $probe.Close()
}
Assert-That 'the port is released after shutdown' $portFree "something is still listening on $port"

# ---------------------------------------------------------------------------
# a closed stdout pipe must not kill the shell (logging is best effort)

# ---------------------------------------------------------------------------
# a closed stdout pipe must not kill the shell (logging is best effort)

Write-Host ''
Write-Host 'closed stdout pipe' -ForegroundColor Cyan
$psiTail = [System.Diagnostics.ProcessStartInfo]::new()
$psiTail.FileName = $exe
$psiTail.Arguments = '--no-browser'
$psiTail.RedirectStandardOutput = $true
$psiTail.RedirectStandardError = $true
$psiTail.UseShellExecute = $false
$tail = [System.Diagnostics.Process]::Start($psiTail)
$tailUrl = $null
for ($i = 0; $i -lt 100 -and -not $tailUrl; $i++) {
    $line = $tail.StandardOutput.ReadLine()
    if ($null -eq $line) { break }
    if ($line.Trim().StartsWith('URL')) { $tailUrl = $line.Trim().Substring(3).Trim() }
}
# Stop reading and close the pipe, then make the shell log again: it must survive
# the failed write instead of panicking with "failed printing to stdout".
$tail.StandardOutput.Close()
if ($tailUrl) {
    $tailMatch = [regex]::Match($tailUrl, '^http://127\.0\.0\.1:(\d+)/$')
    if ($tailMatch.Success) {
        $tailPort = [int]$tailMatch.Groups[1].Value
        $after = Send-Request $tailPort 'GET' "/api/health" "127.0.0.1:$tailPort" @{} $null
        Start-Sleep -Milliseconds 300
        Assert-That 'the shell keeps serving after its stdout pipe is closed' ($after.Status -eq 200 -and $tail.HasExited -eq $false) "status $($after.Status), exited: $($tail.HasExited)"
        $tailQuit = Send-Request $tailPort 'POST' "/api/shutdown" "127.0.0.1:$tailPort" @{} ''
        $tailExited = $tail.WaitForExit(8000)
        Assert-That 'it still shuts down cleanly with no stdout' ($tailExited -and $tail.ExitCode -eq 0) "exited: $tailExited, exit code: $($tail.ExitCode)"
    }
    else {
        Assert-That 'the second shell printed a parsable URL' $false $tailUrl
        try { $tail.Kill() } catch { }
    }
}
else {
    try { $tail.Kill() } catch { }
}

# ---------------------------------------------------------------------------
# The UI scripts, checked as source.
#
# Every other check here talks to a running client, and a running client cannot see
# this class of bug: the page loads, the markup is all present, and the failure is a
# JavaScript error that only shows up as something *not* happening. That is how an
# undeclared `sessionKv` in pages.js went unnoticed - it threw inside the page
# builder, the throw travelled up through `boot()`, and the health poll, the
# one-second tick and the drawer wiring after it never ran at all.
#
# The `node` calls need stderr-not-fatal: this script stops on error, and a lint
# that is *working* writes its findings to stderr.

$webDir = Join-Path $root 'web'
$appJs = Join-Path $webDir 'app.js'
$pagesJs = Join-Path $webDir 'pages.js'

foreach ($pair in @(@('app.js', $appJs), @('pages.js', $pagesJs))) {
    $label = $pair[0]
    $text = [IO.File]::ReadAllText($pair[1])
    Assert-That "$label survives a UTF-8 read intact" ($text -notmatch [char]0xFFFD)
}

$nodeExe = Get-Command node -ErrorAction SilentlyContinue
if ($null -ne $nodeExe) {
    $saved = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    foreach ($name in @('app.js', 'pages.js')) {
        $path = Join-Path $webDir $name

        & node --check $path *> $null
        Assert-That "$name parses as JavaScript" ($LASTEXITCODE -eq 0)

        $lint = Join-Path $PSScriptRoot 'undeclared.mjs'
        $findings = (& node $lint $path 2>&1 | Out-String).Trim()
        Assert-That "$name reads no undeclared name" ($LASTEXITCODE -eq 0) $findings
    }
    $ErrorActionPreference = $saved
}
else {
    Write-Host '  SKIP  node is not installed; the JavaScript source checks did not run' -ForegroundColor DarkYellow
}

# `el` is the persistent `.page` element: the router only empties its children, so a
# listener left on it outlives the page and the next visit adds a second one. One
# click on 开启隧道 then fires one startTunnel() per visit so far, and every one after
# the first is refused with "已经有一个隧道在运行了" by the tunnel the first one had
# just started - a success and an error from a single click. Every delegated listener
# therefore has to be taken off again in `destroy`.
$appText = [IO.File]::ReadAllText($appJs)
$pagesText = [IO.File]::ReadAllText($pagesJs)

$adds = ([regex]::Matches($pagesText, 'el\.addEventListener\(')).Count
$removes = ([regex]::Matches($pagesText, 'el\.removeEventListener\(')).Count
Assert-That 'every delegated listener on .page is taken off again' ($adds -gt 0 -and $adds -eq $removes) "adds $adds, removes $removes"
Assert-That 'the connect page removes its click and change listeners' `
    (($pagesText -match 'el\.removeEventListener\("click"') -and ($pagesText -match 'el\.removeEventListener\("change"'))

# A listener on `document` outlives the page for the same reason one on `.page` does,
# and it is quieter about it: the relay picker puts its "click outside closes me" there,
# and a leftover would keep shutting a list nobody can open, once per visit. Every
# handler added has to be removed by the same name, and the picker has to be shut as
# the page goes.
$docAdded = @([regex]::Matches($pagesText, 'document\.addEventListener\("([a-z]+)",\s*([A-Za-z]+)\)') |
    ForEach-Object { $_.Groups[2].Value })
$docRemoved = @([regex]::Matches($pagesText, 'document\.removeEventListener\("([a-z]+)",\s*([A-Za-z]+)\)') |
    ForEach-Object { $_.Groups[2].Value })
$docLeaked = @($docAdded | Where-Object { $docRemoved -notcontains $_ })
Assert-That 'every document listener is removed by name too' ($docAdded.Count -gt 0 -and $docLeaked.Count -eq 0) `
    ("added: " + ($docAdded -join ', ') + " / removed: " + ($docRemoved -join ', '))
Assert-That 'the relay picker is closed when the page goes' ($pagesText -match 'destroy:[\s\S]{0,1200}?closePicker\(\)')

# The session token is gone; a half-removed one is worse than either state, so no page
# script may read, store or send one.
Assert-That 'no page script handles a session token any more' `
    ((($appText + $pagesText) -notmatch 'x-hongshi-token') -and (($pagesText) -notmatch 'sessionStorage'))

# An asset URL built at runtime still goes through the helper, so the one place that
# knows how a shell asset is addressed stays the one place.
Assert-That 'runtime icon URLs go through the helper' ($appText -match 'assetUrl\("icons\.svg#i-')

# A page that hardcodes a reference instead of building one still works - but it is the
# convention that keeps the helper meaningful, so the sprite goes through it.
Assert-That 'no page script hardcodes an icons.svg reference' ($pagesText -notmatch 'icons\.svg#')

# A status pill's first child is its dot, so `lastElementChild` writes the label into
# the dot and leaves the visible label frozen on its initial words. Every pill write
# has to go through the helper that finds the real label element.
Assert-That 'no pill writes its label into the dot' `
    (($pagesText -notmatch 'lastElementChild\.textContent') -and ($pagesText -match 'function setPill'))

# ---------------------------------------------------------------------------

Write-Host ''
if ($failures.Count -eq 0) {
    Write-Host ("all {0} checks passed" -f $checks) -ForegroundColor Green
    exit 0
}

# Leave nothing behind: a shell that is still running holds the binary open, and
# the next `cargo test` then fails with "failed to remove hongshi.exe" - which
# looks like a build problem and is not one.
foreach ($stray in @($process, $tail)) {
    if ($null -ne $stray) {
        try {
            if (-not $stray.HasExited) { $stray.Kill() }
        }
        catch { }
    }
}

Write-Host ("{0} of {1} checks FAILED:" -f $failures.Count, $checks) -ForegroundColor Red
foreach ($name in $failures) { Write-Host "  - $name" -ForegroundColor Red }
Write-Host ''
Write-Host 'console output was:'
Write-Host $stdout
exit 1
