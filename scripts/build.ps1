<#
.SYNOPSIS
  Build a hongshi shell binary and copy it into a directory.

.DESCRIPTION
  Mirrors hongshi/scripts/build-release.ps1 in shape, deliberately: same platform
  labels (windows-amd64, linux-amd64, linux-arm64, macos-arm64, macos-amd64), same
  artifact naming (hongshi-<platform>-<arch>[.exe]), same binary-format assertion
  before the file is copied anywhere. The download site expects exactly those names -
  the WebUI group on download.html reads /api/download/webui?platform=... and
  server.py documents hongshi-windows-amd64.exe, hongshi-linux-amd64,
  hongshi-macos-arm64.

  -CopyTo should be ..\HongshiMain\download\webui: that is the directory
  HongshiMain/build_static.py reads when it regenerates the site, and copying
  anywhere else produces a file nothing serves. (`public/download/webui`, which this
  help used to name, does not exist.)

  Default is a debug build in this crate's own target/ directory, which is what
  you want while working on the UI. -Release produces the artifact that ships.

  -Platform builds one or more of the labels above. Linux is cross-compiled to static
  musl: the missing `rust-std` is downloaded and unpacked into the sysroot here,
  because `rustup target add` does not work on this toolchain (it came from a v1
  manifest), and the musl targets link with the toolchain's own rust-lld (see
  .cargo/config.toml) so no Docker or WSL is needed.

  macOS is the one target that cannot be cross-built: `rust-std` installs, the crate
  compiles, and the link then fails for want of a Mach-O linker and the macOS SDK.
  Asking for it from a non-Apple host is refused with that explanation rather than a
  linker error. Build it on a Mac, or on a CI job running on `macos-latest`.

.NOTES
  This file is intentionally ASCII-only: Windows PowerShell 5.1 reads .ps1 files
  as ANSI unless they carry a UTF-8 BOM, which silently corrupts non-ASCII text
  and breaks parsing.

.EXAMPLE
  powershell -File scripts\build.ps1
  powershell -File scripts\build.ps1 -Release
  powershell -File scripts\build.ps1 -Release -CopyTo ..\HongshiMain\download\webui
  powershell -File scripts\build.ps1 -Release -Platform windows-amd64,linux-amd64 -CopyTo ..\HongshiMain\download\webui
  powershell -File scripts\build.ps1 -Release -Platform macos-arm64   # on a Mac
#>

param(
    [switch]$Release,
    [string]$CopyTo,
    [string[]]$Platform = @('windows-amd64')
)

$ErrorActionPreference = 'Stop'

# `-File script.ps1 -Platform a,b` hands the whole thing over as one string rather
# than binding an array, while `& .\script.ps1 -Platform a,b` binds it properly.
# Splitting here means both invocations do what they look like they do.
$Platform = @($Platform | ForEach-Object { $_ -split ',' } | Where-Object { $_ })

$root = Split-Path -Parent $PSScriptRoot
$profileName = if ($Release) { 'release' } else { 'debug' }

# A relative -CopyTo is relative to the crate root, not to wherever the shell happens
# to be standing. The paths this is documented with (`..\HongshiMain\...`) are written
# from here, and resolving them against the caller's directory made the same command
# work or fail depending on which folder it was typed in.
if ($CopyTo -and -not [System.IO.Path]::IsPathRooted($CopyTo)) {
    $CopyTo = Join-Path $root $CopyTo
}

$sysroot = (rustc --print sysroot)
$hostTriple = (& rustc -vV | Select-String '^host: ').Line.Split(' ')[1]

# Platform label -> target triple. Keep in sync with the download page and with
# hongshi/scripts/build-release.ps1.
#
# The labels are the `<platform>-<arch>` halves of the published file names, so the
# download site's own spelling is the only one in play: the label `macos-arm64`
# produces `hongshi-macos-arm64`, which is the example server.py documents. (The
# kernel's CI calls its macOS builds `darwin-arm64` and publishes them as
# `hongshic-macos-arm64`; here the label and the file name agree, which is one fewer
# thing to get wrong while reading a directory listing.)
$MATRIX = [ordered]@{
    'windows-amd64' = @{ triple = 'x86_64-pc-windows-msvc';    ext = '.exe' }
    'windows-arm64' = @{ triple = 'aarch64-pc-windows-msvc';   ext = '.exe' }
    'linux-amd64'   = @{ triple = 'x86_64-unknown-linux-musl'; ext = '' }
    'linux-arm64'   = @{ triple = 'aarch64-unknown-linux-musl'; ext = '' }
    'macos-arm64'   = @{ triple = 'aarch64-apple-darwin';      ext = '' }
    'macos-amd64'   = @{ triple = 'x86_64-apple-darwin';       ext = '' }
}

$unknown = $Platform | Where-Object { -not $MATRIX.Contains($_) }
if ($unknown) {
    throw "unknown platform(s): $($unknown -join ', '); expected one of $($MATRIX.Keys -join ', ')"
}

# macOS cannot be cross-built from Windows or Linux, and the failure is worth naming
# here rather than leaving someone to read a linker error.
#
# Rust *compiles* the crate quite happily for an Apple target once the `rust-std`
# package below is unpacked — the compile is target-independent. The link is what
# fails, because two things a Mach-O link needs are not on a non-Apple machine: a
# driver that speaks Mach-O, and the macOS SDK that holds libSystem's symbols and the
# `.tbd` stubs. On this Windows host rustc reports both at once:
#
#     warning: invoking `"xcrun" "--sdk" "macosx" "--show-sdk-path"` ... program not found
#     error: linker `cc` not found
#
# The SDK is not redistributable, so this is not a flag or a package away: it needs a
# Mac, or a CI runner that is one. hongshi/.github/workflows/release.yml already
# builds the kernel on `macos-latest` for exactly this reason.
#
# Matched on the requested labels rather than on `$triple`, which only exists inside
# `Build-One` — an earlier version of this guard tested a variable that was null here
# and therefore never fired, which is exactly the sort of thing a guard is for.
$appleFromHere = @($Platform | Where-Object {
    $MATRIX[$_].triple -like '*-apple-darwin' -and $hostTriple -notlike '*-apple-darwin'
})
if ($appleFromHere.Count -gt 0) {
    throw @"
$($appleFromHere -join ', ') needs a macOS host: this machine is $hostTriple.

Rust compiles this crate for an Apple target quite happily, but linking it does not
work without a Mach-O linker and the macOS SDK, and the SDK may not be redistributed.
Run this on a Mac:

    powershell -File scripts\build.ps1 -Release -Platform $($appleFromHere -join ',')

or add the shell to a CI job on `macos-latest` (hongshi/.github/workflows/release.yml
already does that for the kernel). Nothing else is affected: the Windows and Linux
targets cross-compile from here.
"@
}

# Download the rust-std component for a target and unpack it into the sysroot.
# Same approach as hongshi/scripts/build-release.ps1, and for the same reason.
function Install-TargetStd([string]$triple) {
    if ($triple -eq $hostTriple) { return }

    $stdDir = Join-Path $sysroot "lib\rustlib\$triple"
    if (Test-Path (Join-Path $stdDir 'lib')) {
        Write-Host "  std present: $triple"
        return
    }

    $version = ((rustc --version) -split '\s+')[1]
    $url = "https://static.rust-lang.org/dist/rust-std-$version-$triple.tar.gz"
    Write-Host "  installing std for $triple ..."

    $tmp = Join-Path ([System.IO.Path]::GetTempPath()) "rust-std-$triple"
    Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $tmp | Out-Null

    $tarball = Join-Path $tmp 'rust-std.tar.gz'
    $saved = $ProgressPreference
    $ProgressPreference = 'SilentlyContinue'
    try {
        Invoke-WebRequest -Uri $url -OutFile $tarball -UseBasicParsing
    }
    catch {
        throw "could not download $url - the shell cannot cross-build $triple without it: $($_.Exception.Message)"
    }
    finally {
        $ProgressPreference = $saved
    }

    # Native commands write progress to stderr; PowerShell turns that into a
    # terminating error while ErrorActionPreference is Stop.
    $ErrorActionPreference = 'Continue'
    tar -xf $tarball -C $tmp
    $tarExit = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    if ($tarExit -ne 0) { throw "tar failed (exit $tarExit)" }

    $unpacked = Join-Path $tmp "rust-std-$version-$triple"
    if (-not (Test-Path $unpacked)) { throw "unexpected layout in $tarball" }

    $source = Join-Path $unpacked "rust-std-$triple\lib\rustlib\$triple"
    if (-not (Test-Path $source)) { throw "no rustlib for $triple inside the component" }

    New-Item -ItemType Directory -Force $stdDir | Out-Null
    Copy-Item (Join-Path $source '*') $stdDir -Recurse -Force
    Write-Host "  installed std into $stdDir"
}

# Read the machine type out of a built artifact.
#
# Throws rather than returning "unknown": this runs before anything is copied, and
# a wrong-target build is far cheaper to catch here than after it has been uploaded
# and linked from the download page.
function Get-BinaryKind([string]$path) {
    $bytes = [System.IO.File]::ReadAllBytes($path)

    if ($bytes[0] -eq 0x4D -and $bytes[1] -eq 0x5A) {
        $peOff = [System.BitConverter]::ToUInt32($bytes, 0x3C)
        $machine = [System.BitConverter]::ToUInt16($bytes, $peOff + 4)
        switch ($machine) {
            0x8664 { return 'PE x86_64' }
            0xAA64 { return 'PE aarch64' }
            default { throw ("$path is PE machine 0x{0:X4}" -f $machine) }
        }
    }

    if ($bytes[0] -eq 0x7F -and $bytes[1] -eq 0x45 -and $bytes[2] -eq 0x4C -and $bytes[3] -eq 0x46) {
        $machine = [System.BitConverter]::ToUInt16($bytes, 18)
        switch ($machine) {
            0x3E { return 'ELF x86_64' }
            0xB7 { return 'ELF aarch64' }
            default { throw ("$path is ELF machine 0x{0:X4}" -f $machine) }
        }
    }

    # Mach-O, 64-bit, little-endian: the magic reads CF FA ED FE on disk. The CPU type
    # is the u32 that follows, where CPU_ARCH_ABI64 (0x01000000) is or-ed onto the base
    # type - 7 for x86_64 and 12 for arm64.
    #
    # A fat/universal binary starts with CA FE BA BE instead and is deliberately not
    # accepted: each download page row is one file for one architecture, so a universal
    # one would be published twice under two names.
    if ($bytes[0] -eq 0xCF -and $bytes[1] -eq 0xFA -and $bytes[2] -eq 0xED -and $bytes[3] -eq 0xFE) {
        $cpu = [System.BitConverter]::ToUInt32($bytes, 4)
        switch ($cpu) {
            0x01000007 { return 'Mach-O x86_64' }
            0x0100000C { return 'Mach-O arm64' }
            default { throw ("$path is Mach-O cputype 0x{0:X8}" -f $cpu) }
        }
    }

    throw "$path is neither PE nor ELF nor Mach-O"
}

function Build-One([string]$label) {
    $triple = $MATRIX[$label].triple
    $isHost = $triple -eq $hostTriple
    Write-Host "building hongshi-shell ($profileName, $label) ..."
    Install-TargetStd $triple

    # The host is built without `--target` so it lands in `target/<profile>/`, which
    # is where `cargo run`, `verify.ps1` and the kernel's own `core/` discovery all
    # look. Passing `--target` for the host too would put it in `target/<triple>/`
    # and quietly leave every one of those pointed at a stale binary.
    $cargoArgs = @('build', '--bin', 'hongshi')
    if (-not $isHost) { $cargoArgs += @('--target', $triple) }
    if ($Release) { $cargoArgs += '--release' }

    Push-Location $root
    try {
        # cargo prints its progress to stderr, and PowerShell turns a native
        # command's stderr into a terminating error while ErrorActionPreference is
        # Stop. Relax it for the call and judge the build by its exit code instead.
        $ErrorActionPreference = 'Continue'
        & cargo @cargoArgs
        $buildExit = $LASTEXITCODE
        $ErrorActionPreference = 'Stop'
        if ($buildExit -ne 0) { throw "cargo build failed (exit $buildExit) for $label" }
    }
    finally {
        Pop-Location
    }

    $name = "hongshi$($MATRIX[$label].ext)"
    $exe = if ($isHost) {
        Join-Path $root "target\$profileName\$name"
    } else {
        Join-Path $root "target\$triple\$profileName\$name"
    }
    if (-not (Test-Path $exe)) { throw "no binary at $exe" }

    # Confirm the machine type here rather than after it has been uploaded.
    $kind = Get-BinaryKind $exe

    $kb = [math]::Round((Get-Item $exe).Length / 1KB, 0)
    Write-Host ("  {0}  {1} KB  ({2})" -f $exe, $kb, $kind)

    if ($CopyTo) {
        if (-not (Test-Path $CopyTo)) { New-Item -ItemType Directory -Force $CopyTo | Out-Null }
        $target = Join-Path $CopyTo "hongshi-$label$($MATRIX[$label].ext)"
        Copy-Item $exe $target -Force
        Write-Host ("  copied to {0}" -f $target)
    }
}

foreach ($label in $Platform) { Build-One $label }

Write-Host ""
Write-Host "next:"
Write-Host "  powershell -File scripts\verify.ps1        # start it, talk to it, stop it"
