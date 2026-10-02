# hongshi shell (`hongshi`)

The official WebUI shell for the hongshi kernel: **a local server plus your own browser.**

Double-click it and it starts a server on `127.0.0.1`, opens the page in whatever browser you
already use, and from there you drive the tunnel. There is no bundled webview and no Chromium —
that is most of why the release binary is 2.3 MB and why the interface can be developed by editing
HTML and pressing F5.

```
hongshi.exe            the WebUI shell          <- this crate
   │  spawns
   ▼
hongshic.exe           the kernel, a separate download
```

This crate is deliberately **separate from `../hongshi`** (the kernel and relay). Nothing here is
linked into the kernel, and the kernel does not know this program exists: the two are two
downloads that ship together on the same page.

There is a third artifact, and it is not a third download:
[`HongshiAndroid`](https://github.com/hongshionline/HongshiAndroid)
compiles this crate for `aarch64-linux-android` and runs it inside an Android app, with a `WebView`
pointed at the same loopback URL. The page cannot tell the difference, and neither can anything
below — the interface, the site proxy, the node list and the latency probe are the same code. That
work is why `Shell::start` exists separately from `run`, and why `net`, `config` and `kernel` each
grew a way for a host to say where its own files are and what to fetch with: on Android the
executable's directory is the read-only `/system/bin`, and the fallback HTTP client refuses
`https://` outright while `api_base` is `https://hongshi.site`.

**Nothing about the desktop build changed for any of it.** Every seam has a default, the defaults
are what the binary you download uses, and this crate is still at zero dependencies.

---

## The interface

Chinese, with a **top bar**: the mark alone on the left (which links home), the destinations in the
middle as **words only** with the current one underlined, and the account slot on the right. Quit
and the connection pill sit in a small status bar under the board.

Both halves of the bar lost something on purpose. The mark used to carry 红石联机 / Redstone Online2
beside the icon and the destinations used to carry a glyph before every two-character label; the
icon already draws the brand and the words are already two characters, so both second parts were
decoration taking up the horizontal room the bar needs.

The log panel has **no switch in the bar** any more. It opens from the page that needs it (查看日志
on 联机) and closes with Escape, which `drawerOpen` installs a document-level listener for.

The bar replaced a left sidebar that collapsed to an icon rail with its state in `localStorage`.
Four destinations did not earn 232px of every screen, and the rail's collapsed state was a
preference worth remembering only because the layout made it one.

| Destination | State |
|---|---|
| 首页 | a games board: a contribution heat map of time played on the left, a rotating game card on the right. Headed 游戏日历 with a calendar glyph rather than by a page title |
| 联机 | **one decision and one button**: pick a relay (with a stability reading), accept the detected port, press 开启房间. The address is handed over in a dialog |
| 一起玩 | a placeholder that says 此功能暂不对外开放 |
| 设置 | service address, proxy, node-cache lifetime, default port, and 关于 (version, update check, download) |

云存档 and 租聘服 are still real routes and still render their placeholder page, but they no
longer have a link in the bar: 一起玩 is the one unbuilt destination a player is meant to see.

**帮助 is not in the bar either, and that is a decision.** Every question in it is reached from the place
that raises it: 什么是游戏端口？ under the port field on 联机 (and in the first-run guide), 朋友怎么加入房间？
and 常见问题：朋友连不上怎么办？ under the address once a room is open — which is the only moment those two
questions exist. A fifth tab would be a place to *browse*; these are places to *land*, one click after
something blocked the user, and the four things in the bar are all destinations.

`/help` is the page and `/help/<slug>` is a question in it. Every question is one entry in `HELP_TOPICS`
plus one article function in [`pages.js`](web/pages.js) — a slug, a name, a one-line blurb, whether it
has screenshots, and the article — so the second and third question were data edits rather than
rewrites. The row of topic chips renders only once there is more than one question, which is now the
case; before, a tab bar with a single tab was furniture.

**什么是游戏端口** answers in three parts, and the middle one is the part somebody follows with the game
open on the other monitor:

1. what a port is — IP is the building, the port is which room; a LAN world is visible only to the same
   Wi-Fi, and the tunnel is what puts it somewhere a friend can reach;
2. how to see your own — ESC → 世界选项… → set 多人游戏 to 局域网 → 应用更改. One screenshot per step,
   taken from a real session;
3. where to put it, with a button back to 联机.

It also says the thing that line of chat makes people worry about: the red 「无法转发端口 … 路由器上未启用
UPnP」 printed right under the port is the *game* failing to punch its own hole, and it is not a problem
here — this client relays through a server instead of forwarding a port.

**朋友怎么加入房间** is written for the friend, not the host, because the host never sees those three
screens — the host forwards the link, or forwards the address and reads it out. Three steps (多人游戏 →
直接连接 → paste the address → 加入服务器), and then the two facts that cause most of the failures: the
address is *two* halves (`cd.hongshi.site:27913` — the part after the colon is this room's port, and
dropping it is the most common mistake), and the room only exists while 红石 is showing that address.
Its second half is about the window in which the room is up: closing it, exiting the world, sleeping
the machine and losing the network all end the same way for the friend on the other side, and a reopened
room has a *new* address.

**朋友连不上怎么办** is written backwards from the screen: somebody opens it with a game window behind it
showing one sentence, so every case leads with **that sentence verbatim** in the monospace face the game
uses — `Connection refused: getsockopt`, `Unknown host`, `无效的玩家档案公钥签名` — and only then says
what it means. The order is by frequency, and the one case that needs the *host* to act carries a 较麻烦
tag, because the friend reading the page cannot fix that one alone.

The cases themselves came from the support deck the team kept before this page existed
(`常见问题.pdf`, a seven-page PowerPoint). Two things in it were out of date and were fixed rather than
carried over: **port forwarding is not something this product needs** — relaying is the whole point, and
the old "check your router's port mapping" line sent people to do work that cannot help — and
"无效会话" and "无效的玩家档案公钥签名" are one cause with two wordings, not two causes. The page opens
with three self-checks (is the address still on the host's screen, do both sides have the same version
and mod count, was the address copied whole) because those three answer most reports before the list is
needed, and it ends with the official Q group (497060189) as a button that copies the number — reading
six digits off a screen and typing them into a phone is where people give up.

Every illustrated answer opens with where the screenshots came from. For the three walkthroughs that is
**Minecraft Java 版 26.3** with **LAN World Plug-n-Play (mcwifipnp)**, because "does my game look like
this?" is the question the reader has while looking at somebody else's menu bar, and the version-mismatch
answer depends on it. The troubleshooting page says something different and more necessary: its six
screenshots were collected from months of feedback, so they are **visibly from different versions** (one
is 1.21.11, two are phone-client layouts) — and the error *text* is what the reader is matching against,
not the window around it. All twelve are the client's own assets (532 KB together) because a help page
that needs the internet to show a picture is not a help page on the machine that has no internet.

The tunnel is **not a destination of its own**; it is driven from 联机, which is where the user
asked for it. 首页's 服务 section — which used to carry the same three cards — has been removed
along with 每日资讯 and 当前会话, and the space below the games board is deliberately blank while
the daily content that replaces them is designed. `/api/daily-news` is still served and still
tested; nothing on the Rust side changed when the page stopped asking for it.

**首页** leads with the games board. The heading is a calendar glyph and 游戏日历 rather than the
page's own name over a sentence describing the board already on screen.

The heat map is one cell per day and one column per week over the last 18 weeks — a contribution
calendar applied to "how much did I play", with the weekday gutter, the month labels along the top
and a ring on today. **The data is real**, and the graph is the visible end of the session history:

* `kernel::reap` writes one record to `hongshi.sessions.json` when a kernel exits — see
  [`src/sessions.rs`](src/sessions.rs). The *kernel* records the session, not the page, so a browser
  closed mid-session cannot lose the time and a page reload cannot double-count it.
* `/api/sessions?days=126` aggregates that file to one row per day, and **the level buckets live in
  Rust** (`sessions::level`) rather than in the page, so the graph and any other reader of the same
  file cannot disagree about what a colour means.
* The page folds the **live** tunnel into today's row while a room is open, so the square fills in as
  you play instead of jumping when you stop. That is the one place the scale is duplicated in
  JavaScript, and it is commented at both ends.
* A session shorter than 60 seconds is not recorded at all: a tunnel that never reached the relay is
  not a play session, and a mark on the calendar for a failure would be worse than no mark.

**联机** is one decision and one button. It used to be a form — 转发方式 (中转 / P2P), 协议
(TCP / UDP), 本机地址, a relay listbox and a tunnel card — with only the first option of each ever
selectable, which is not a choice but a reading assignment. 转发方式 and 协议 did not stop existing
(the kernel still only does relayed TCP); they went because stating them at the user as if they were
decisions is worse than saying nothing.

The two controls that remain are the two the user can answer:

* **中转服务器**, with a stability reading from the shell's own probe. `ok` is 延迟稳定 in green,
  `slow`/`ping` is 较稳定 or 仅能 ping 通 in amber, and nothing answered is 断联 in a deliberately
  bright red. The distinction between "the control port answered" and "only ICMP did" is the whole
  point: a node you can ping but not tunnel through is not a usable node, and calling it 稳定 would
  be a lie the user pays for with a dead address they handed to a friend.
* **本地游戏端口**, which is not really a choice — it is a fact about where the game is listening, so
  it is *detected* and shown, and the field exists to correct the detection rather than to demand an
  answer. An empty field is filled in; a field the user has touched is left alone, unless they press
  the **refresh button** beside it, which is the user asking for it to be overwritten. That button is
  one glyph in a 44px square, not a labelled button: a text label at the field's own height was wide
  enough to squeeze the field it belongs to, and a smaller pill beside a 44px field is the one shape
  that looks wrong however the row is arranged. The word it would have carried is in the hint under
  the field, which says where the port came from and says to press the refresh button when nothing was
  found; the `aria-label` keeps it for a screen reader. And it is not decoration:
  detection is a snapshot of a machine that changes, and "no game running" is the answer until the
  user starts one.

  The detection itself has three steps, in [`src/ports.rs`](src/ports.rs):

  1. **`25565`–`25569`, held by a Java process.** The vanilla default and the ports a second or third
     world lands on. One `netstat`, and the search is usually over.
  2. **Any other port in 10000–65535 held by a Java process.** This is the step that earns its keep: a
     modpack launcher or a hand-edited `server.properties` puts the game somewhere else, and the only
     thing distinguishing that socket from the other thirty is *which process owns it*. Identified by
     cross-referencing listening ports with process names — on Windows `netstat -ano` (33 ms measured)
     plus `tasklist` (254 ms) **only when a name is actually needed**; on Linux one `ss -ltnp`, with
     `netstat -ltnp` as the fallback. Matching is on the image *stem*, so `javaw.exe` and `java8.exe`
     count while `javascript-tool` does not.
  3. **`25565` as a floor**, reported as `fallback` rather than as a finding, and the page says so in
     words: "we did not find your game, this is the default" and "we found your game" are different
     claims and the user is about to hand this number to somebody else.

  There is a known gap in step 2, stated here so it is not rediscovered as a bug: a server running
  under a launcher's bundled JRE whose image is *not* named `java` is not recognised by name, and the
  search falls through to step 3. That is why step 3 exists and why it says what it is. The other way
  to recognise a Minecraft server — speaking its own protocol at it with a Server List Ping — was the
  first design and is strictly more informative; it is not what this does because the client already
  owns one child process and its stdout, and a second protocol stack is a large thing to own in the
  one part of the product that has no dependencies.

Starting a room does **not** open the log panel. The address is the thing the user came for, so it
arrives in a dialog, and the dialog opens *immediately* in a pending state rather than after a wait:
a fixed wait cannot tell "not yet" from "not ever", and the first version of this flow announced
failure while the page behind it already showed the address. Closing a room is a toast, not a
pageful of terminal output.

The game card is a screenshot with a scrim under the title, and the rotation is already written
even though there is one game: the interval only starts for more than one, so the second game is a
data edit rather than a rewrite.

In the interface the program calls itself **客户端** and calls `hongshic` the **内核**. The word
"shell" is used in this README, in the crate name and in the code, and nowhere a user can see it:
to someone who is not a programmer, a "shell" wrapping a "kernel" reads as a command prompt.

### The first launch: a spotlight, and one notice

Two things run at boot, in this order — the guide, then the update notice, and never both at once.

**Onboarding is a spotlight.** One real control is lifted out of a dimmed page and one sentence sits
beside it. It replaced four dialogs with a diagram each, which was the wrong shape for the job: a
dialog describes the interface *somewhere the interface is not*, so the user read about a button and
then had to go and find it. Each step is now a target plus a sentence, and the tour never invents a
surface of its own:

1. the 联机 tab in the top bar — 点击切换联机页. Advancing is *the click itself*, so the tour follows
   the user instead of asking them to confirm they read something.
2. the relay picker — 选择一台离你最近的服务器…, with 知道了 to go on. This is the one step whose action
   is deliberately not the way forward: clicking that control opens a list, and advancing on that
   click would move the ring down to the port row while the list the user was just told to read is
   still hanging open under it.
3. the port row — the detection is explained, with a link out for anyone who does not know what a
   game port is.
4. 开启房间 — advancing is pressing the button. That button is `disabled` until a kernel is installed,
   and **a disabled button dispatches no click event at all**, so this step also carries 知道了 and,
   when the kernel is missing, a sentence naming the 自动下载内核 button that has to come first.

Escape, 跳过 and reaching the end are the user saying they are done, and the flag is written for those
only. A click on the dimmed page ends the guide too — a guide that will not go away is worse than no
guide — but it is *not* remembered, so a misclick does not cost somebody the rest of the tour, and
neither does the step-3 link, which walks off to read the help before step 4 has been seen. The flag is
also treated as a first run rather than as a preference: a kernel already installed is evidence of a
previous run, so it counts as "seen" and is recorded as such.

**The update notice is the answer to "you should not have to go looking".** The client asks the
official site for the latest version about two and a half seconds after boot and speaks only when there
is a **newer** version the user has not been told about. Three answers come back — newer, 已是最新, and
"could not check" — and exactly one of them is worth a modal at startup; the settings page (关于) is
where all three belong. It is once per *version* rather than once per launch, because a notice that
returns every morning is a notice people learn to dismiss without reading.

### The tenth launch

[`src/launch.rs`](src/launch.rs) counts launches of the **client** — `hongshi.launches.json` beside the
executable, bumped once per process start and reported as `launches` in `/api/health`. Counted in the
page's `localStorage` instead it would be a count of *that browser*: reset by clearing site data, doubled
by a second browser, and never advanced at all by a build whose page is opened somewhere else. It is
deliberately not a field in `settings.json` either, because that file is rewritten whole every time the
settings form is saved, and a counter that forgets is worse than no counter — the only thing anybody
does with this number is ask whether it is a round one.

Every tenth launch, and only then, the shell says thank you and points at 爱发电. Two conditions decide
it, and both are about not being rude:

* **When** — when the *room* dialog is dismissed, by 知道了, by Escape or by a click on the backdrop. Not
  at startup, which would be asking a stranger, and not on a launch where no room ever came up, which
  would be thanking somebody for a session that did not happen: the address arriving is what arms it.
* **Once per run** — a second room in the same evening does not ask again.

**下次一定** closes it; **去赞助** opens `https://ifdian.net/a/RedstoneOnline` in a new tab, because the
room the user just opened has to stay on screen behind it. The address is printed in full above the
buttons rather than hidden behind one of them: a tip link nobody can read before clicking is a link
people do not click.

The relay is a **listbox**, not a native `<select>`. It was a table of rows that were each secretly a
radio button — a table of facts that was really one control — so it became the control it was. The
first version of that was a `<select>`, and it lasted one round: **a `<select>` cannot be styled.**
On Windows the popup is drawn by the OS, so `border-radius`, `padding` and layout on an `<option>` are
ignored, and every row came out as one flat string with the name, the latency and the state run
together. What this markup wants — rounded rows, the node on the left and the numbers on the right —
is not something an `<option>` can be asked for.

So both halves of the control are drawn from one list of rows (`pickerRows()`), which is what keeps
the button and the open list from disagreeing about what exists:

```
[ 南京  nj.hongshi.site                    ● 36 ms  可建隧道 ]   <- the button, closed

  ( 自动选择   挑延迟最低的可用节点         ● 11 ms  可建隧道  )   <- the list, open
  ( 南京       nj.hongshi.site              ● 36 ms  可建隧道  )
  ( 成都       cd.hongshi.site              ● 11 ms  可建隧道  )
  ( 广州       gz.hongshi.site              ● 29 ms  不可达    )
```

It is a real listbox and not a `div` that looks like one: `role="combobox"` / `role="listbox"` /
`role="option"`, `aria-expanded`, `aria-selected`, and `aria-activedescendant` for the keyboard
cursor. ↑/↓ open it and move, Enter and Space choose, Escape closes, a click outside closes, and
focus returns to the button. The chosen row and the keyboard cursor look different on purpose — one
is what the tunnel will use, the other is where the arrow keys are.

The open list follows the toast's elevation language rather than inventing one: a `--plate` surface,
a `--rule` border, 16px corners, and **no shadow**, because the design system has exactly one shadow
and it is not this one.

The list comes from the official `/api/server/list`, and each node's latency is **measured by the
shell**, because a browser can neither send ICMP nor open a raw TCP connection, and a cross-origin
`fetch` would be blocked anyway.

The list is read **once, when the client starts**, and every visit to 联机 renders that same
snapshot — with the time it was taken under the buttons. It used to be read and re-probed on every
visit, which was a round of ICMP/TCP per relay on a page you might open four times in a minute, for a
list that changes when an operator adds a machine. 「重新测速」and 「刷新节点列表」are what ask for
fresh data; the shell caches the upstream read for `node_cache_seconds` so even those do not hammer
the site. A probe re-renders the picker, so the selection is restored by value and falls back to
自动选择 if that node is gone — a selection pointing at an option that no longer exists would leave
the control blank.

That measurement is **ICMP first, the control port second**, and the result says which one answered:

| State | Meaning |
|---|---|
| 可建隧道 | the control port answered — this number is the tunnel's own round trip |
| 仅能 ping 通 | ICMP answered, port 8080 did not: reachable but unusable |
| 不可达 | neither answered |

The distinction is not decoration. `nj.hongshi.site` answers a ping in 39 ms while refusing 8080 at
times, and calling that "不可达" would blame the user's network for a node that is simply not
accepting tunnels. 自动选择 only ever picks a node in the first row of that table, since those are
the ones a tunnel can actually be built on.

**主页** leads with Minecraft 资讯, then 服务: 联机隧道, 云硬盘 and 云服务器 as three cards of one shape
(the tunnel's detail swaps between "nothing running" and its live address), then 当前会话. The news
grid is `auto-fill` columns so it always spans the width, and a picture leads the card full-bleed.

The news is **Minecraft's own launcher feed**, `https://launchercontent.mojang.com/news.json`, and it
replaced a hongshi endpoint (`GET /api/daily_news`) that this client used to proxy. The page never
talks to Mojang itself — a `fetch` from `http://127.0.0.1:<port>` to another origin is cross-origin,
and `connect-src 'self'` says so — so the shell fetches it, and does three things the page would
otherwise have to:

* **Trims it.** The file is 64 KB of a hundred entries and changes at most once a day; the page is
  given the first three, as a 1.3 KB answer, and the shell caches those for `node_cache_seconds` so
  opening 主页 repeatedly does not re-read the whole thing.
* **Makes the pictures absolute.** Entries carry `/images/…`, which the page cannot resolve — it is
  served from loopback, so a relative path would be looked for on the client. `img-src` therefore
  carries the Mojang origin, not just 服务地址.
* **Normalises the shape.** Every field the page reads is a plain string; upstream, the image is an
  object and half the entries carry a `tag` the others do not. A field that is sometimes an object is
  what made the old reader guess.

The parser is deliberately narrow — it walks the `entries` array by brace depth, skipping string
bodies so a `}` in a headline cannot end an entry early — and it lives in `src/site.rs` with unit
tests against a fixture that carries the awkward parts. Anything it cannot read costs an entry or a
field, never a wrong value: **an empty list is rendered as "没读到任何一条资讯", not as an error**, and
an entry with no title is dropped rather than drawn as a blank card.

**The feed is frozen.** `news.json` has a `Last-Modified` of April 2024 and its newest entry is
2024-01-16, so what the page shows is the launcher's last published batch, not today's news. That is
the upstream's state, not this client's: point `NEWS_URL` somewhere else and the rest still works.

**设置** covers the service address, the proxy, the node cache lifetime and the default port, writes
them next to the executable, and ends with 关于: the running version, a 检查更新 button, and a download
link. The check asks `GET /api/webui/version` and has three answers, not two — 有新版本, 已是最新, and
**无法检查**. A `404` is the third: it means the site has published no version, and reporting that as
"已是最新" would be a claim nobody made.

The running build is **测试版 v0.5.0**. The channel is one constant (`crate::CHANNEL`) because
`--version`, the About panel and the site's endpoint are three places a user can compare, and two of
them disagreeing is worse than either being wrong.

## Status: phase 2 — the kernel is wired up

Working now: the whole interface, the local server, the site proxy, the node
list, the latency probe, settings persistence, quit — and **the tunnel itself**. The client finds
`hongshic`, spawns it, streams its stdout into the log panel, reads `endpoint=` off that stdout and
puts the address on the card. `POST /api/tunnel/start|stop` drive it; `GET /api/kernel` reports what
it is doing.

Two endpoints exist for the 联机 page and neither needs an argument it cannot default:
`GET /api/ports/detect[?default=N]` reports the local game port it found (and what it looked at),
and `GET /api/sessions[?days=N]` reports the recorded history rolled up to one row per day. Both are
reads of something the machine already knows, so a malformed query gets the normal answer rather
than an error — a bad window should draw the usual window.

The kernel is looked for in `core/` **beside the executable and in the working directory**, under
either its plain name (`hongshic.exe`) or the name it is published as
(`hongshic-windows-amd64.exe`) — someone who downloaded it by hand will not have renamed it. When
there is none, the 联机 page says which file is missing, where it looked and for which platform, and
carries a **自动下载内核** button. That button goes through the *shell*, not through a browser link:
`POST /api/kernel/download` fetches the build from the official endpoint
(`GET /api/download/client?platform=…&arch=…`) and writes it into `core/`, because a browser download
would land in the user's Downloads folder and the client would still not find it. The platform comes
from the client's own build rather than a user-agent string: the kernel runs on the same machine, so
the client is the authority. When the install lands, `kernelStore.refresh()` is what makes the notice
disappear and 开启房间 come alive — the kernel store is the single thing that decides whether a kernel
exists, so nothing else tries to guess it from a button click.

The integration contract is [hongshi.site/api.html](https://hongshi.site/api.html), and three things
in it shape the code:

* **The endpoint is read off stdout** — split on `endpoint=`, take up to the next whitespace. The
  contract says everything before it is decoration and may change, so nothing else in that line is
  parsed for meaning — and "decoration" includes terminal colour, which is asked away with
  `NO_COLOR=1` and stripped regardless. A kernel that colours its output is not a different kernel.
* **A new process is a new tunnel on a new port.** The previous address is dropped the moment a new
  one starts; a stale endpoint would send players to a port that no longer exists.
* **The exit code is the whole result**: `0` the tunnel ended, `1` it never got one. A process *this
  client* killed also exits 1, so a user-requested stop reports 已关闭 rather than repeating the
  contract's "the relay was unreachable" — a stop is not a failed tunnel.

## Build and run

```powershell
cargo run                        # start it and open the browser
cargo run -- --no-browser        # just start it; the URL is on the console
cargo test                       # 104 unit tests + 20 end-to-end tests
powershell -File scripts\verify.ps1   # 188 acceptance checks against the real binary

# the artifacts that ship
powershell -File scripts\build.ps1 -Release -Platform windows-amd64,linux-amd64 `
     -CopyTo ..\HongshiMain\download\webui
```

`pwsh` is PowerShell 7 and is not installed on every machine this is built on; the scripts are
ASCII-only and run under Windows PowerShell 5.1, so `powershell -File` is the form that works
everywhere. `& .\scripts\verify.ps1` from a shell already in the crate root is the shorter one.

`scripts/build.ps1` names its artifact the way the download site expects —
`hongshi-windows-amd64.exe`, `hongshi-linux-amd64` — matching the `webui` group on
`HongshiMain/public/download.html`. `-CopyTo` writes them where the site actually reads them:
**`HongshiMain/download/webui/`**, which is the directory `HongshiMain/build_static.py` copies into
`dist/download/webui/` when it regenerates the site. (An earlier version of this file said
`public/download/webui`; that directory does not exist, so artifacts copied there were never served.)
A relative `-CopyTo` resolves against the crate root, not the caller's directory. `-Platform` takes
one or more labels; the host is built without `--target` so it lands in `target/release/` where every
other tool looks, and the rest are cross-compiled.

**Linux is cross-compiled to static musl**, so one file runs on any distribution. The missing
`rust-std` is downloaded into the sysroot by the script — `rustup target add` does not work on this
toolchain, which came from a v1 manifest — and the musl targets link with the toolchain's own
`rust-lld` (see `.cargo/config.toml`), so no Docker, WSL or zig is involved. The script asserts the
machine type of every artifact before copying it, because a wrong-target build is much cheaper to
catch here than after it has been uploaded.

**macOS is the one target that cannot be cross-compiled**, and it is worth knowing why rather than
rediscovering it. `rust-std` for `aarch64-apple-darwin` installs from the same place the musl one
does, and the crate then *compiles* — the compile is target-independent. The **link** is what fails,
because a Mach-O link needs a driver that speaks Mach-O and the macOS SDK that holds libSystem's
symbols, and neither is on a non-Apple machine:

```
warning: invoking `"xcrun" "--sdk" "macosx" "--show-sdk-path"` ... failed: program not found
error: linker `cc` not found
```

The SDK may not be redistributed, so this is not a flag or a package away. Asking `build.ps1` for a
`macos-*` label from a non-Apple host is refused with that explanation instead of a linker error.
Two ways out, both of which work:

```powershell
# on a Mac
powershell -File scripts\build.ps1 -Release -Platform macos-arm64,macos-amd64 `
     -CopyTo ..\HongshiMain\download\webui
```

or a CI job on `macos-latest`, which is what `hongshi/.github/workflows/release.yml` already does for
the kernel — that workflow's own comment says it: *"macOS is built here rather than locally because
Apple Silicon requires a code signature, and only a macOS linker produces one."* The shell now has a
repository of its own, [`hongshionline/hongshi-webui`](https://github.com/hongshionline/hongshi-webui),
which is where that job would go; `Cargo.toml` points at it.

`Get-BinaryKind` in `build.ps1` understands Mach-O as well as PE and ELF, so an Apple artifact is
checked the same way the others are: magic `CF FA ED FE`, then the CPU type (`0x01000007` for x86_64,
`0x0100000C` for arm64). A fat/universal binary is deliberately rejected — each download-page row is
one file for one architecture.

Current artifacts for **测试版 v0.5.0**:

| Artifact | Size | Format |
|---|---|---|
| `hongshi-windows-amd64.exe` | 2257 KB | PE x86_64, console |
| `hongshi-linux-amd64` | 2448 KB | ELF x86_64, static musl, no PT_INTERP |
| `hongshi-macos-arm64` / `hongshi-macos-amd64` | — | not built here; see above |

Both are staged for upload by `scripts/build.ps1 -Release -CopyTo dist`, which names them the way the
download page reads them and asserts the format before copying. The Linux figure is *larger* than the
Windows one because musl is linked in statically — the price of a binary that runs on any distribution
without a libc to match.

Most of the last 800 KB is the artwork and the help pages, all embedded rather than fetched: the game
banner (98 KB), the window background (17 KB), and twelve screenshots across three help articles
(532 KB), plus two subset CJK faces (725 KB) that no web font CDN could serve for a page that must work
with no network. They are WebP rather than JPEG because the same picture is 172 KB as JPEG at this
quality, and the binary is downloaded twice — once for the desktop build and once inside the Android
app. The games board's 640×360 cut is deliberately *not* embedded: nothing requests it, and everything
in `web/asset/` is both requestable and paid for in download weight. It lives in `artwork/` beside the
crate until something reads it.

### Options

Every option has a default; running it with no arguments is the intended use.

| Option | Default | Meaning |
|---|---|---|
| `--port <N>` | `0` (the OS picks a free one) | Loopback port to listen on |
| `--web-dir <DIR>` | next to the executable, then `./web` | Serve the UI from disk instead of the copy inside the binary |
| `--no-browser` | off | Print the URL instead of opening a browser |
| `--verbose` | off | Print `DEBUG` lines |
| `-v`, `--version` / `-h`, `--help` | | Print and exit |

Exit codes: `0` orderly shutdown, `1` runtime failure (a busy port), `2` usage error.

## Developing the UI

The page is six files in `web/` and they are **compiled into the binary** with `include_str!`. If a
`web/` directory is found next to the executable (or wherever `--web-dir` points), files are served
from disk instead — so during development you edit HTML/CSS/JS and press F5, with no rebuild:

```powershell
cargo run -- --web-dir web        # edit web/*, press F5 in the browser
```

F5 is the whole loop, which is worth stating because it did not work for a while: the page used to
be gated behind a `?token=…`, and a reload is a fresh request for `/` with no query string, so the
browser was answered `403` before `app.js` ran. Nothing in the test suite noticed, because every
request in it was built by hand with the token in it. See the security model above.

The console always prints which copy it is serving (`UI  D:\Hongshi2\shell\web` versus
`UI  embedded in the binary`), so "why is my edit not showing up" answers itself.

### Two traps in `web/`, both of which have already cost a working file

**Line endings are mixed, and PowerShell 5.1 decodes a BOM-less UTF-8 file as ANSI.** `main.css` and
`pages.js` are UTF-8 with CRLF; `app.js`, `index.html`, `app.css` and `icons.svg` are UTF-8 with LF.
One `Get-Content … | … | Set-Content` round trip through the ANSI code page turns every Chinese
string in a CRLF file into mojibake, and the round trip is lossy, so the bytes cannot be inverted.
Edit these files with a UTF-8-aware tool, and if one is damaged anyway, the last build still has a
clean copy: `include_str!` embedded it in the executable.

```powershell
node scripts/extract-embedded.mjs target\debug\hongshi.exe pages.js recovered.js
```

**A JavaScript error in a page is silent.** The markup can all be present and correct while the page
builder throws part-way through; there is no syntax error for `node --check` to find. That is how an
undeclared `sessionKv` in `pages.js` went unnoticed — and because the router called the builder bare,
the throw travelled up through `boot()` and cancelled the health poll, the one-second tick and the
drawer wiring after it, leaving only a status pill frozen on "连接中…". Two guards now exist:

* `node scripts/undeclared.mjs web/app.js web/pages.js` finds names that are read but never declared.
  `cargo test` and `scripts/verify.ps1` both refuse to pass while it reports one.
* `renderPage()` in `app.js` catches a throwing page, shows the error in the page area, and keeps the
  rest of the client running.

The page follows the site's design system in `../HongshiMain/DESIGN.md`: pill controls, one
`drop-shadow` in the whole page, and the fifteen-segment dust line as the signature element.

### The board is red

It is a **bright, saturated red**, not a grey board with a red accent, and that is a different
problem from the one the old palette solved. Two things the grey scheme leaned on do not survive.

**Text can no longer retreat by opacity.** A white at 60% over a red ground is not dimmer white, it
is *pink* — the ground bleeds through the glyphs and every secondary label turns a colour nothing
else uses. The four tiers are therefore painted, picked so each step is a real step down in
luminance over `--plate` (`#7E141C`):

| token | value | on `--plate` |
|---|---|---|
| `--fg` | `#FDF1F3` | 13.5:1 |
| `--fg-soft` | `#F2C2C9` | 9.0:1 |
| `--fg-dim` | `#DA939D` | 6.6:1 |
| `--fg-faint` | `#A85C67` | 3.5:1 — labels, not prose |

The surfaces are three heights of one red — `--ground: #4A0A10`, `--plate: #7E141C`, `--rule:
#A8242D` — and `--wash: #5F0E15` is a recessed surface *inside* a plate. A translucent white would
have been the obvious way to write `--wash` and it is wrong for the same reason as the text: white
over red is pink, and a pink "pressed row" is a colour the palette does not otherwise have.

**The window carries artwork.** `web/asset/lowpoly.webp` — a 1920×1080 low-poly field in **neutral
grey**, facets from 28 to 122 of 255 — is the `body` background, `fixed` and `cover`, under a single
`--scrim` (`rgba(46, 7, 12, 0.62)`). One wash over the window rather than one per card: the cards
already carry `--plate`, and stacking a second wash on them makes the board look dirty. The scrim
exists because the artwork's own contrast is enough to break a paragraph of `--fg-soft` over its
lightest facets.

The wash is also what makes a **neutral** cut usable at all, and that is the trade this asset makes: a
grey field has no hue of its own, so the oxblood wash is what turns it into this board — the tint and
the darkness are one decision made in one place instead of two baked into a file. Its weight follows
the artwork rather than a taste. The red scene this replaced ran 27–48 of 255 (mean 38) and took 0.5;
the neutral cut is twice as bright (mean 77), and at 0.5 its lightest facets came out *above*
`--plate` — the board brighter than the cards, which is the elevation backwards. At 0.62 the worst case
over the lightest facet is `--fg` 10.4:1, `--fg-soft` 7.3:1, `--fg-dim` 4.7:1, and the facet stays
under the cards.

Both masters are in `artwork/`: `lowpoly.png`, which the first WebP was cut from, and
`lowpoly-neutral.png`, which replaced it. Neither is in `web/asset/`, which is exactly the set of files
the browser can ask for and which are all `include_bytes!`-ed into the binary. A file nobody requests in
that directory is a served path and download weight for nothing, which is the rule `verify.ps1` enforces
by name — it caught the neutral master sitting there while its WebP was being cut. It is worth knowing
why the WebP is **13.0 KB** for a 1920×1080 image: the cut is flat facets, a shape WebP is very good at,
and the PNG it came from carries an alpha channel that is 255 in every single pixel — a quarter of that
file was spending itself on a channel nothing reads, and the conversion drops it.

**The fire palette stopped being an accent set.** The five steps are still stored as *roles* rather
than as swatches — `--signal` (`#FF5A33`) is the one accent, `--signal-hot` (`#FF7452`) and
`--signal-deep` (`#E01F2D`) its hover and pressed steps, `--signal-dim` (`#C02A33`) a quiet wash, and
`--signal-ink` (`#8B0000`) a ground for white text — but a signal red on a red ground is nearly the
same colour, so "this is lit" has nothing left to say. Everything that has to read as *active* — the
current destination, the focus ring, the top of the heat ramp, a primary button — uses the brightest
step (`#FF5A33`, a near-orange), which separates from the ground by **luminance rather than hue**.
That is what makes it read as bright and saturated instead of muddy.

The rule that survives from the grey scheme: **a fire colour is never text on a surface.** It is a
fill, a border, an indicator or a graphic; words are always one of the four tiers above.

`--heat-0` … `--heat-4` are the one place these run as an actual ramp. The bottom step is a plate
tint rather than a grey — there is no grey any more — and the top step is **amber `#FFD166` rather
than red**, because a red "most played" on a red card is invisible; the top of the scale has to leave
the family to be the top. The same amber rings today.

The terminal keeps a near-black screen with a deep-red tint (`--screen: #2A0709`) rather than a red
panel: the log is a terminal, and the one thing it must not look like is a card.

### Type: two bundled faces, one per script

**Nunito** for Latin and digits — a rounded variable 400-700 face, embedded as a 38 KB
subset (`web/fonts/nunito-latin.woff2`). **Resource Han Rounded** for Chinese, also
embedded, and this one is cut down by `scripts/build-cjk-subset.py`.

The Chinese face is the interesting half. Every rounded Simplified family is either
absent or a display face — the two rounded CJK families on Google Fonts cover 21/93
and 17/93 of this interface's characters, measured — so the only real option is a full
CJK face, and Resource Han Rounded is 14 MB *per weight*. Subsetting is what makes it
affordable, and subsetting is only safe if the character set comes from the source
rather than from someone's memory of it, because a string added later would render as
tofu. The two weights therefore carry different sets:

| Face | Characters | Size | Why |
|---|---|---|---|
| `han-rounded-regular.woff2` | every character in `web/` and `src/`, plus GB2312 level 1 | 612 KB | the common set is insurance for Chinese this client did not write: the kernel's own console lines reach the log panel, and 服务地址 is user-typed. The news is English now, but the wide set is cheap next to the face and a string that falls back to a system font mid-sentence is worse than a bigger file |
| `han-rounded-bold.woff2` | every character in `web/` and `src/` | 97 KB | the only Chinese this interface renders at 600 or above is text it wrote itself |

### The monospace stack keeps a CJK family in it

`--font-mono` is the terminal's and the numeric slots' face, and it used to be Latin-only — which
meant every Chinese character in one of those slots fell all the way through to whatever the system
calls 等宽/宋体. The visible symptom was 服务 · 联机隧道's **当前没有隧道** being the one string in the
whole interface in a non-rounded face, next to cards whose text was rounded. `"Han Rounded"` now sits
in the stack *before* the generic `monospace`: Latin and digits still take the real monospace faces,
which come first and win per glyph, and Chinese lands on the rounded one instead of a lottery.
Anything that renders Chinese through `--font-mono` — `.usage-value`, `code`, `.input--mono`,
`.row-sub`, and the log terminal — is covered by that one line.

The common set is generated from GB2312's own layout rather than kept as a data file of
3755 characters. Re-cut both after changing any user-visible string:

```powershell
python scripts/build-cjk-subset.py --regular <RHR-Regular.ttf> --bold <RHR-Bold.ttf>
```

Both faces are SIL OFL 1.1, with their licences beside them in `web/fonts/`. The
system stack stays behind them for anything the subsets do not carry, and
`font-display: swap` means the system face shows first rather than nothing.

### Coming and going

Every page is rebuilt from scratch into the same `.page` element, so an animation declared on that
element ran **exactly once, on page load** — every navigation after it was a hard cut, and the
interface looked like it had no transition at all. `show()` now removes the class, forces a reflow
and adds it back, which replays the animation: the incoming board blurs from 7px into focus over
0.34s. `prefers-reduced-motion` turns it off.

The log panel has **no switch of its own**. It used to: a `position: fixed` button in the window's
top-right corner, later moved into the top bar, kept in step with a second close button inside the
drawer's header. Both are gone. The panel is opened by the page that needs it — 查看日志 on 联机, and
the tunnel card before that — and closed with **Escape**, which `drawerOpen` installs as a
document-level listener because the drawer is not focused when it opens and a listener on it would
never fire.

That leaves two ways out and no visible control, which is the trade the user asked for when the bar
button went. It is defensible here and would not be in a panel the user can get stuck behind: the
drawer is a side sheet, not a modal, so the board stays visible and usable beside it, and navigating
away closes it because the page that owns it is destroyed. `drawerOpen` still sets `aria-hidden` on
the drawer so the hidden panel is out of the accessibility tree, and the drawer keeps its own
`[hidden]` rule so it does not paint on load.

The bar's account circle is the only control left in the top bar's right-hand group, and it is
inert by design: it draws the empty-avatar state so the bar's geometry is settled before the page
behind it exists.

Nothing polls for its own sake. The relay list is read once at startup; the kernel is polled at 1 Hz
only while a tunnel is alive, at 0.2 Hz while an open panel waits for one, and **not at all**
otherwise; the 创建于 / 已运行 counters tick every ten seconds rather than every second, because they
are formatted coarsely ("2 分钟前") and a per-second timer is a per-second wake-up for a number that
reads the same either way.

## Security model

This is a server on a port that any process on the machine can reach, and any web page the user has
open can try to reach. Three things stand between those and the shell:

1. **Loopback only.** Binds `127.0.0.1`, never `0.0.0.0`.
2. **Same-origin and loopback-host checks.** `Origin`, when present, must equal the request's own
   `Host`; `Host`, when present, must be `localhost` or a loopback address. The second check is the
   DNS-rebinding defence: a rebound name resolves to `127.0.0.1` while `Origin` and `Host` are both
   the attacker's name, so the same-origin check passes and the loopback-`Host` rule is what refuses.
   A browser navigation carries neither header on its first request, which is why the page is
   reachable at all.
3. **No oracle, and no forged console lines.** A refused caller gets `403` for every path, existing
   or not — a `404` for a stranger would be a free "does this path exist" oracle. Request paths are
   sanitized before they are logged, because refusals are logged too: a caller that is turned away
   still reaches that code path, and without sanitizing it could inject a newline and fabricate a
   console line, or an ANSI escape and repaint the terminal the user is told to trust.

**There is no session token**, and its removal is worth understanding rather than rediscovering.
There was one: 32 hex characters from the OS CSPRNG, required on every route — including
`/favicon.ico` and every stylesheet, sprite and script, which is why the served page carried the
token into its own asset references as it went out, and why a stylesheet's `url("icon.png")` had to
be rewritten on the way past. It bought a boundary against other *programs* on the machine and cost a
page that died on every reload: a refresh re-requests `/` with no query string, so the shell answered
its own interface with `403` before a line of `app.js` ran. The same-machine threat it covered is now
accepted outright; the two checks above still cover the caller that can be made to act without the
user knowing — a web page the user has open.

A random port is chosen by default, which is convenience rather than security: a local process can
find the port, and with no token it can drive the API. That is the deliberate trade, and it is why
"loopback only" is the first line of this list rather than a footnote.

The shell also makes outbound requests (the node list, the launcher news, the latency probes). Those
go through **WinHTTP**, so they use the machine's own certificate store and proxy configuration;
the settings page can override the proxy or turn the system one off. The page cannot make them
itself — a `fetch` from `http://127.0.0.1:<port>` to `https://hongshi.site` is cross-origin — which
is why the shell proxies, and why the CSP stays at `connect-src 'self'`.

Deliberately not defended against, because the threat model is loopback and the operator chose it:
a symlink inside the web root is followed, and another program on the machine can drive the API.

## Layout

```
Cargo.toml            no dependencies at all, on purpose
.cargo/config.toml    crates.io via the USTC mirror (this machine's global config
                      points at another project's vendor directory)
src/
  lib.rs              startup, banner, shutdown, the run() entry point
  options.rs          CLI parsing, pure so it can be tested
  http_server.rs      HTTP/1.1 over std::net: parsing, routing, the origin checks, shutdown
  kernel.rs           the hongshic child process: spawn, stdout pump, endpoint, exit state
  assets.rs           embedded assets, the on-disk override, path validation, Range parsing
  site.rs             the site: node list + cache, the latency probe; the Mojang news reader
  net.rs              WinHTTP over FFI (Windows) or plain TCP elsewhere — no TLS dependency
  config.rs           settings file: parse, clamp, save
  launch.rs           how many times this client has been started, for the tenth-launch thank-you
  sessions.rs         the session history: one record per tunnel, and the daily rollup
  ports.rs            finding the local game port, so the user does not have to know one
  browser.rs          opening the default browser per platform
  util.rs             percent-decoding and ISO-8601 timestamps
  log.rs              one-line console logging; sanitized values, best effort, never fatal
web/
  index.html          the shell: top bar, page container, status bar, log drawer
  icons.svg           every icon as a <symbol>, referenced by <use href="icons.svg#i-…">
  main.css            the design system: tokens, top bar, status bar, board, drawer, terminal
  app.css             page components: heat map, game card, news, usage, node list, tunnel summary, forms
  app.js              runtime: API wrapper, router, dialogs, the first-run guide, terminal, settings
  pages.js            one function per route, including 帮助 and its three articles
  asset/              exactly what ships: lowpoly.webp (the window background), the game banner and
                      the twelve help screenshots. Every file here is embedded in the binary and
                      requestable by the browser — `artwork/` holds what is neither
artwork/              the masters and the cuts nobody requests: lowpoly.png and lowpoly-neutral.png
                      (the two window backgrounds, red and neutral), the 864×864 key art, and the
                      640×360 hero thumbnail. Kept in the repository, out of the served tree
                      and out of the binary. (The other masters — six PNGs of 1.5 / 0.6 / 15.6 / 1.9 /
                      1.1 / 1.0 MB plus the 7-page `常见问题.pdf` the six error screenshots came
                      from — are deliberately not in the repository at all: they can be taken again.)
tests/shell.rs        20 end-to-end tests: spawn the binary, talk to it over a socket
examples/             manual probes: shutdown, first request, connection cap, half-open hold,
                      kernel terminal colour
scripts/build.ps1     build + assert the binary format + optional copy to the download tree
scripts/verify.ps1    188 acceptance checks against the real binary
```

## Why no HTTP framework

The server is ~500 lines over `std::net`, and that is a decision rather than an accident. It is
reached only over loopback, it serves four small files and four JSON endpoints, and it lives in a
binary handed to end users — so hyper + tokio + tower (or a hand-rolled event loop) would cost more
in build time, binary size and supply-chain surface than it saves in lines. Threads are the right
concurrency model at this scale: a browser opens a handful of connections, and a thread blocked on
a socket read cannot starve the accept loop.

What it deliberately does not implement: chunked request bodies, TLS, pipelining, multi-range
responses. Each is either unused by a browser on loopback or answered with a legal fallback (a
multi-range request gets the whole file).

## Bugs worth remembering

Most were found by end-to-end tests, by driving the real binary, or by an adversarial review — and
several would have shipped in a build that only had unit tests. They are written down here because
the reasoning is not obvious from the final code. Two were found by *using the interface in a
browser*, which no test in this repository does, and one by the acceptance script being the slow
reader it was warning about.

**Every accepted socket was non-blocking, so the whole server was subtly broken.** This is the one to
read first, and it was the hardest to find. `TcpListener::set_nonblocking(true)` is how the accept
loop stays able to notice a shutdown request — but **on Windows the accepted socket inherits that
flag**, and `serve_connection` never cleared it. The consequence was not a crash: it was that every
read returned `WouldBlock` immediately, so a well-formed request could be answered with a 408, a
malformed one was dropped with an RST because the handler closed a socket that still held unread
data, and the connection budget could never fill (every slot was released microseconds after it was
taken). The fix is one line at the top of `serve_connection`:

```rust
stream.set_nonblocking(false)?;   // accept inherits the listener's flag on Windows
```

It was found only because a connection-cap test kept refusing to work: the cap looked broken, and
chasing *that* surfaced the real defect. A feature that is hard to test is often telling you
something more important than itself.

**The page could not load its own CSS or JS, and every test missed it.** A browser resolves
`<link href="main.css">` against the page URL and **drops the query string**, so it asks for
`/main.css` with no token while the page itself was fetched with one. Since every route is gated, the
stylesheet and the script both came back `403`: the shipped UI was unstyled, `app.js` never ran, the
session card stayed on "checking…", and Quit did nothing. It passed 38 unit tests, 16 end-to-end
tests and 41 acceptance checks because **all of them fetched `/main.css?token=…` — a request no
browser ever makes.** The fix wrote the token into the page's own asset references as it was served
(`tokenize_asset_links`), which kept the gate on every route rather than exempting two files — with
the on-disk `web/` layer, an exemption is not "two files", it is "whatever is in that directory".
The test that now guards it derives the URLs from the page's own markup, exactly as a browser would,
and `scripts/verify.ps1` repeats it. A real browser was then driven against a live shell to confirm:
65 CSS rules loaded, `app.js` populated the session card, Quit stopped the process and freed the port.

That fix was correct and is now gone, along with the token it served: the page is no longer rewritten
on the way out, so the bytes a developer edits are the bytes the browser receives. The *test* it left
behind is the part that mattered, and it is why the rewrite's removal was noticed rather than
assumed — it still derives its requests from the markup.

The general lesson is not about assets: **a test that constructs its own input can only confirm what
it already assumes.** Every fetch in the suite was written by the same hand that wrote the server,
and it encoded the same wrong mental model.

**A refusal must not be a 200.** `Response::json` constructs a 200 by design, so `forbidden_api()`
— which returned a JSON error through it — answered a refused request with status 200. Any caller
checking `response.ok` (which is exactly what `web/app.js` does) would have read the refusal —
then worded as "bad or missing session token" — as success. The same mistake was still live for an
unknown `/api/…` path, which returned 200 with `{"error":"no such endpoint"}`; that is a 404 now.
The wording changed when the token did; the status code was the bug.

**Logging must not be able to fail the process.** `println!` panics when the write fails, so a
closed stdout pipe killed the shell: `hongshi | head`, a console window the user closed, or a
launcher that stopped reading. The symptom was an "orderly shutdown" exiting with code 101. Every
line the shell prints now goes through `log`, which writes with `write_all` and drops the error.
(On Unix this was never about SIGPIPE: `std` ignores it, and it is `println!`'s own unwrap that
panics.)

**Requests could forge console lines.** The request path is percent-decoded before it is logged, and
refusals are logged too — so any web page the user had open could reach that code path with no token
at all and inject a newline, producing a console line indistinguishable from a real one ("INFO
forged URL http://evil/ status=200"), or an ANSI escape to repaint the terminal. Values are now
sanitized before they reach a log line.

**An idle socket was treated as a request that timed out.** A browser opens several speculative
connections per page load and leaves them idle; each one sat until the 5-second read timeout and was
then both answered with a 408 *and* logged as `method=-  path=-  status=408`. One ten-minute session
produced about a hundred of those lines, which buries the events the console exists to show — and the
console is the window the user is told to keep open. A timeout that has taken **zero bytes** on the
request line is now closed in silence; a client that sent part of a request still gets its 408 and
still appears in the log. The distinction is only visible in how much was read, which is why the
"nothing at all" case has to be decided inside the line reader rather than by its caller.

Two more, both found by using the interface rather than by a test:

* **A news image is only as reachable as the service address.** The `img-src` policy was the literal
  pair `https://hongshi.site http://hongshi.site` under a comment saying "the service address", so
  changing 服务地址 — to a mirror, or to a local stub while working on the UI — silently dropped every
  news picture while the text still rendered. The configured address is on the list now, alongside
  the news' own publisher. The value is user-typed and goes into a response header, so only the
  characters that can appear in a host and port survive: a `;` there would end `img-src` and start a
  directive of the attacker's choosing, and a newline would end the header.
* **A status pill's label is not its last child.** A pill starts with `.pill-dot`, so
  `pill.lastElementChild.textContent = "已保存"` wrote the words into the *dot* and left the visible
  label reading whatever it was born with — the 关于 pill said 未检查 no matter what the update check
  concluded, and 保存 said 未保存 after a successful save. Any pill whose dot is small and
  `overflow: hidden` looks fine while being completely wrong. Every pill write goes through
  `setPill()`, which looks for the real `.pill-text` element, and `verify.ps1` refuses to pass while a
  `lastElementChild.textContent` write exists in `pages.js`.
* **An open log panel sat on top of the board.** The drawer took `--drawer-w` of screen but the board
  only reserved `--drawer-w - --side-w` — 188px of padding against a 420px panel — so the panel
  covered the right-hand 188px of every page at all times. On 联机 that is the end of the tunnel
  card's address row and the latency column of the node table, and it reads as the card being partly
  hidden behind the log. The board now reserves the whole panel and only falls back to a sheet over
  the content on a window too small for both.
* **A page that never got its title back.** The catch-all route wrapped `soonPage` and discarded its
  return value (`routes.soon(el, name)` with no `return`), so the page object was lost: the tab title
  stayed bare and the placeholder pages got none of their own lifecycle hooks. `renderPage` catching
  a throwing page is not the same thing — a page that returns nothing looks like a page that has
  nothing to say.
* **Logging could block the server.** `println!` panicking on a closed pipe was found and fixed long
  ago; a *full* pipe is the same hazard with the opposite symptom. Every served request writes a
  console line, so a stdout nobody is draining fills its buffer after a few dozen lines and the next
  write parks the calling thread — the accept loop included. Measured: with stdout on a pipe that
  was never read, 200 requests hung and `/api/shutdown` never took effect. Logging now runs on its
  own thread behind a bounded queue; a caller hands over a line and returns, and when the reader is
  too slow the lines are dropped and counted rather than queued without limit. The client reports
  how many it dropped on the way out, so a support transcript with a gap in it says so.
  The acceptance script was the slow reader that found this, and it now drains the console on a
  background read for the whole run — before, it left the pipe to fill and then asserted against a
  truncated transcript.
* **The kernel's terminal colour broke the address.** Two user-visible symptoms, one cause. The
  kernel logs through `tracing`, which colours its output whenever the `ansi` feature is on and
  `NO_COLOR` is unset or empty — and **not**, as is easy to assume, only when stdout is a terminal.
  So a kernel spawned with a pipe is coloured on every machine that has not already set that
  variable, and it writes `\e[3mendpoint\e[0m\e[2m=\e[0mcd.hongshi.site:53190`: the literal
  `endpoint=` never appears, the address is never lifted out, and the card sits on "还没有分配地址"
  while the tunnel is genuinely up. The user's reasonable next move is to press 开启隧道 again — and
  is told a tunnel is already running, because one is. That second message was reported as its own
  bug; it was the first one wearing a hat.

  Two fixes, at two layers. The spawn now sets `NO_COLOR=1`, which is the intended way to ask; and
  every line is stripped of escape sequences before anything reads it, because the kernel is a
  separate program whose build this client does not control and a parser must not depend on a
  courtesy being honoured. Two further lessons are in the parser: the sentence around the field
  **contains the word** ("hand the `endpoint` above to the players"), so every occurrence is tried
  and only a name followed by `=` counts; and a name must be a whole word, so `myendpoint=` is not
  `endpoint=`.

  The reasoning for stripping was first "verified" as unnecessary, and that is the part worth
  keeping. This shell's own environment has `NO_COLOR=1`, the spawned kernel inherited it, and the
  probe dutifully reported that a piped kernel does not colour its output — about a kernel that
  colours on every ordinary machine. `examples/kernel_colour_probe.rs` now prints whether `NO_COLOR`
  is set before it measures anything, and runs the child both ways, so the next person to ask this
  question gets an answer about the product instead of about their own shell.
* **A stop that had not stopped yet.** `kill` only asks. `stop` used to answer from the state
  *before* the process was reaped, so it could reply `running: true` about the kernel it had just
  killed — and a client that then refuses the next start looks broken while being correct. `stop`
  now waits, briefly and boundedly, for the process to actually go. There is an end-to-end test for
  start → stop → start with no pause, because the window between dead and reaped *is* the bug.
* **The session token made the page die on every reload.** Both of the bugs below came out of driving
  the real interface in a real browser, and neither was visible to 134 passing acceptance checks.

  The token was checked on **every** route, including `/`. A browser navigation is the only request
  that cannot carry a header, so the token travelled in `?token=…` — and `app.js` stripped it out of
  the address bar on load, into `sessionStorage`, so a screenshot would not carry a session. That is
  the whole bug: a reload re-requests `/` with no query string, the shell answered its own page with
  `403` *before a single line of `app.js` ran*, and the fallback that read `sessionStorage` was
  unreachable by construction. The page could not be refreshed, bookmarked or restored by the
  browser, and the README's own instruction — "edit `web/*`, press F5" — described something that had
  never worked.

  The fix was to delete the token rather than patch around it. It bought one thing, a boundary
  against other *programs* on the same machine, and cost the primary user gesture. What is left is
  the check that was always doing the security work anyway: a request is refused by where it came
  from (`Origin` against the request's own `Host`, and a loopback `Host`), not by what it carries.
  The lesson is in the shape of the testing: every request in the suite carried a token, because the
  same hand wrote the server and the tests, so all of them asked a question no browser asks. The page
  now has a check that derives its asset URLs from the page markup and a check that requests `/`
  twice, because "the second request" is the one that was broken.
* **One click started one tunnel per visit to the page.** `connectPage` delegates its `click` and
  `change` handlers onto `el`, and `el` is `.page` — the same element for the life of the tab, which
  the router only empties (`pageEl.textContent = ""`) between navigations. The delegation is
  deliberate: the tunnel card is rebuilt whenever the kernel state changes, so binding the buttons
  themselves would lose the handler on the first redraw. What was missing is the other half —
  `destroy()` unsubscribed from the stores and stopped the ticker, and never took the DOM listeners
  off. So each visit to 联机 added one more, and one click fired `startTunnel()` once per visit so
  far: one tunnel started, and every request after the first refused with 已经有一个隧道在运行了,
  which the user reads as an error from the button that just succeeded. The panel showed
  `启动内核 … ×3` for a single kernel.

  Found by counting requests: the console had `POST /api/tunnel/start` **three** times (409, 200,
  409) against a single click on the third visit. Fixed by naming both handlers and removing them in
  `destroy`, and by a `verify.ps1` check that the number of `el.addEventListener` calls equals the
  number of `el.removeEventListener` calls — which was confirmed to fail when the two lines are
  deleted again, because a guard that cannot fail is not a guard. The same work found a smaller
  doubling: `watch(true)` and `drain()` are called together when a tunnel starts, and both fired a
  `/api/kernel/log` request with the same `since`, so every server line was written to the panel
  twice. `drain` now shares the request already in flight.

Two deliberate design notes that fall out of the same work:

* **Shutdown is a flag, not a shared handle.** Two attempts to let `/api/shutdown` close the
  listener both hung the shell: first the handle got an empty slot and closed nothing, then sharing
  the real one through an `RwLock` starved the writer, because the accept loop re-locks the read
  side every iteration and `std`'s `RwLock` can leave a writer waiting indefinitely. The listener is
  now owned by the accept loop, made non-blocking, and polled every 10 ms; shutdown is one atomic,
  and the "shutdown watcher" thread that coordinated nothing is gone. The process then waits
  `SHUTDOWN_GRACE` before exiting, so the response that asked for the shutdown is actually
  delivered instead of being cut off by an RST.
* **A refused request still needs its response delivered.** A small response can be coalesced with
  the FIN, and Windows turns `close()` into an RST when the receive buffer still holds unread bytes
  — the peer's pending data is discarded and the client that sent a malformed request never sees the
  400. `finish()` drains what the peer already sent (bounded at 64 KB, since a hostile client
  reaches this path too) and then shuts the write half down.

**A dialog can be dismissed by the click that opened it.** The room dialog appears about 200 ms after
开启房间 is pressed — the shell has to spawn the kernel and wait for the relay to hand out an address —
so the second click of a double-click, or an automated click that retried because the button it aimed at
had just been covered by an overlay, lands on the backdrop and closes the message before it can be read.
Found while testing the tenth-launch thank-you, which kept vanishing mid-flow; the dialog is not
supposed to be dismissible by a gesture that predates it. `Element.prototype.remove` wrapped for one run
and a stack trace later, the culprit was the backdrop handler. It now ignores clicks for the first
350 ms, which costs a deliberate dismissal nothing.

**A 200 is not the same as a working page.** The help page shipped to a blank screen: the top bar drew,
the status pill sat on 连接中…, and `#page` stayed empty. The cause is that `/help/port` is the first
route in this client with **two segments**, and every asset reference was relative — in `index.html` and
in `Shell.assetUrl`. A browser resolves `href="main.css"` against the page URL, so on that page it asked
for `/help/main.css`, got a 404, and arrived with no stylesheet, no script and no icons; `window.Shell`
was never defined at all.

What makes it worth writing down is how invisible it was. `/help/port` answers 200 with the whole
document in it, so every server-side check passed — including the acceptance script's "a browser can
fetch each asset" loop, which derives its requests from the page *as served at `/`*: one segment deep,
where relative paths happen to work. The only tell was a rendered page with nothing in it.

References are absolute now and `assetUrl` prefixes the slash, held by two checks: the Rust suite
compares the served page with `web/index.html` byte for byte, and `verify.ps1` refuses any relative
`href`/`src` in the markup. The second one found a second thing on its way in — two checks had been
asserting `href="main.css"` as a stand-in for "the page is not rewritten in flight", a literal that
stopped meaning anything the moment the paths changed; both now compare against the file, which is what
their names said all along.

**The guide built a layer that ate the click it existed to pass through.** The spotlight's overlay is
`pointer-events: none`, which is the decision the whole thing turns on — the user is told to press the
real 联机 link and the real 开启房间 button, so the page underneath has to stay live. The first version
then added `.guide-hit`, a small transparent rectangle over the lifted control, "so the click reaches
it". It did the opposite: the rectangle sat *on top of* the link and took the click itself, so step 1
could never be finished by doing what it said, and a click on the dimmed page never dismissed anything
either, because the overlay that was listening for that click had `pointer-events: none` too. **Two
symptoms, one wrong mental model: an element that sits on top of a control competes for the click
rather than conducting it.** The layer is gone, and `verify.ps1` fails if the name comes back.

**"Was this click aimed at the guide?" cannot be asked of the DOM.** With the layer gone the question
is a listener on `document`, and the first version answered it with `lifted.contains(event.target)`.
That is wrong here for a reason specific to this interface: **every page replaces its own contents on
each store publication**, and when the replacement lands between the press and the release the browser
retargets the click at the common ancestor. A press the user aimed squarely at the relay picker arrives
with `#room-body` as its target, `contains` says no, and the tour ends mid-step — which is how it was
found, by happening on one run and not the next. The coordinates are what the user aimed at, so the
coordinates are what is tested now, against three boxes: the card, the lifted control, and any list
that control has opened (a popover belongs to the thing that opened it and hangs outside its box).

**A card that points at a list has to get out of its way.** The relay list opens *downwards* out of the
control step 2 points at, and the card was placed below its target like every other step — squarely on
top of the rows the user had just been told to read. The rows were still in the DOM and still clickable
in principle; they simply could not be clicked, which is the kind of bug a screenshot does not show and
a `querySelector` cheerfully confirms is fine. Step 2 places above, and step 4 places at the top of the
page for the same class of reason at the other end of the window: there is no room under 开启房间, and
"above" lands on the two controls that step's own sentence names — the button itself, and the
自动下载内核 button it tells the user to press first.
