/*
 * hongshi shell — pages.
 *
 * One function per route. Each returns `{ title, destroy?, onTick?, onHealth? }`;
 * `destroy` releases intervals a page created, and the shell calls it before
 * drawing the next page.
 *
 * Every page is built as a string of HTML and handed to `innerHTML`, with values
 * escaped through `Shell.escapeHtml`. That is a deliberate trade at this size:
 * one document, no build step, and the escaping is in one helper rather than
 * spread through a framework. Nothing here interpolates a value that has not been
 * through it.
 */

(function () {
  "use strict";

  var S = window.Shell;
  if (!S) return;

  var icon = S.icon;
  var esc = S.escapeHtml;

  /* --------------------------------------------------------------- helpers */

  function pageHead(title, sub) {
    return '<header class="page-head"><h1 class="page-title">' + esc(title) + "</h1>" +
      (sub ? '<p class="page-sub">' + sub + "</p>" : "") + "</header>";
  }

  function section(title, iconName, body, note) {
    // `icon` here must not be a parameter name. It used to be, which shadowed the
    // icon helper in this scope: `icon(iconName)` then called a *string*, and
    // every section whose fourth argument was missing threw. Section one survived
    // by coincidence — its body landed in the shadowing parameter's slot, so the
    // call "worked" and its markup silently lost the note and gained a body in the
    // title's place.
    return '<section class="section">' +
      '<h2 class="section-title">' + icon(iconName) + esc(title) + "</h2>" +
      body +
      (note ? '<p class="section-note">' + note + "</p>" : "") +
      "</section>";
  }

  /** A notice box. `tone` is "warn" for something the user should act on. */
  function notice(iconName, html, tone) {
    return '<div class="notice' + (tone ? " notice--" + tone : "") + '">' +
      icon(iconName) + "<div>" + html + "</div></div>";
  }

  /**
   * Set a status pill's state and its words.
   *
   * The label must be its own `.pill-text` element. A pill starts with
   * `.pill-dot`, so `pill.lastElementChild` addresses the *dot* — writing the label
   * there put the words inside the dot and left the visible label showing whatever
   * it was born with. The 关于 pill therefore kept reading 未检查 no matter what the
   * check concluded, and the 保存 pill kept reading 未保存 after a successful save.
   */
  function setPill(pillEl, stateName, text) {
    if (!pillEl) return;
    if (stateName) pillEl.dataset.state = stateName;
    var label = pillEl.querySelector(".pill-text");
    if (label) label.textContent = text;
  }

  /** The markup for a status pill, so every one of them gets a real label element. */
  function pill(id, stateName, text) {
    return '<span class="pill"' + (id ? ' id="' + id + '"' : "") +
      ' data-state="' + stateName + '"><span class="pill-dot"></span>' +
      '<span class="pill-text">' + esc(text) + "</span></span>";
  }

  function emptyBox(text) {
    return '<p class="empty empty--box">' + esc(text) + "</p>";
  }

  /**
   * How a build is named on screen: `测试版 v0.2.0`.
   *
   * The channel comes from the client (`/api/version` reports it beside the number)
   * rather than being written here, so `--version` on the console, the About panel
   * and the site's own endpoint cannot drift into three different answers.
   */
  function versionLabel(body) {
    var version = (body && body.current) || "";
    if (!version) return "版本未知";
    var channel = (body && body.channel) || "版本";
    return channel + " v" + version;
  }

  function soonPage(el, name) {
    var titles = { cloud: "云存档", hosting: "租聘服", together: "一起玩" };
    var details = {
      cloud: ["把世界存档放到红石云端，换台电脑接着玩。", [
        "上传与下载存档，带版本历史",
        "与联机隧道联动：开隧道时自动同步",
        "按存档体积计费的云硬盘"
      ]],
      hosting: ["不想自己开机器，就让红石替你开。", [
        "一键开出带公网地址的常驻服务器",
        "按地区就近选择，可随时升降配置",
        "与联机页共用同一份节点列表"
      ]],
      together: ["把朋友的服务器聚到一个列表里。", [
        "收藏常用的联机地址",
        "看到谁在线、能不能进",
        "记住每个世界的模组与版本"
      ]]
    };
    var entry = details[name] || ["这个服务还在路上。", []];
    el.innerHTML = pageHead(titles[name] || "敬请期待", entry[0]) +
      '<div class="soon">' +
      // The icon carries its own status, so the tag travels with it rather than
      // sitting in a corner of the card where it reads as decoration.
      '<div class="soon-mark">' + icon("rocket", "icon--lg") +
      '<span class="pill pill--tag" data-state="warn"><span class="pill-dot"></span>' +
      '<span class="pill-text">实验性</span></span></div>' +
      "<h3>此功能暂不对外开放</h3>" +
      "<p>请您关注<strong>每日资讯</strong>以获得最新消息。下面的功能是计划中的样子，" +
      "不代表已经可用。</p>" +
      (entry[1].length
        ? '<ul class="soon-list">' + entry[1].map(function (line) {
            return "<li>" + esc(line) + "</li>";
          }).join("") + "</ul>"
        : "") +
      '<div class="soon-actions">' +
      '<button type="button" class="btn btn--ghost" data-go="home">回到主页</button>' +
      '<button type="button" class="btn btn--ghost" data-go="home-news">看每日资讯</button>' +
      "</div>" +
      "</div>";

    el.querySelectorAll("[data-go]").forEach(function (button) {
      button.addEventListener("click", function () {
        S.go("home");
        // The second button lands on the news, which is the answer to "when".
        var target = document.getElementById("news");
        if (button.dataset.go === "home-news" && target) {
          target.scrollIntoView({ behavior: "smooth", block: "start" });
        }
      });
    });
    return { title: titles[name] || "敬请期待" };
  }

  /* ------------------------------------------------------------------ home */

  function homePage(el) {
    // Minecraft 资讯 leads. It is the only part of this page that changes on its own
    // and the only part with something to say when nothing is set up yet, so it goes
    // above the fold; the services and the session facts are reference material.
    el.innerHTML =
      pageHead("主页", "红石联机的本地客户端。这里有 Minecraft 的最新消息，以及你的服务与用量。") +
      '<section class="section section--first">' +
      '<h2 class="section-title">' + icon("info") + "Minecraft 资讯" +
      '<span class="pill" id="news-state" data-state="busy" style="margin-left:auto">' +
      '<span class="pill-dot"></span><span class="pill-text" id="news-state-text">读取中…</span></span>' +
      "</h2>" +
      '<div class="news" id="news"></div>' +
      '<p class="section-note" id="news-note"></p>' +
      "</section>" +
      section("服务", "link", '<div class="usage" id="usage"></div>',
        "三项服务共用同一个账号体系与节点列表。联机已经可用，云存档与租聘服还没上线，"
        + "所以它们现在显示「不可用」。") +
      section("当前会话", "info", '<dl class="kv" id="session-kv"></dl>');

    var state = el.querySelector("#news-state");
    var stateText = el.querySelector("#news-state-text");
    var note = el.querySelector("#news-note");
    var news = el.querySelector("#news");
    var sessionKv = el.querySelector("#session-kv");

    function renderState(title, body, stateName, stateLabel) {
      news.innerHTML = '<article class="news-card news-card--state">' +
        '<p class="news-title">' + esc(title) + "</p>" +
        '<p class="news-body">' + body + "</p>" +
        "</article>";
      if (state) state.dataset.state = stateName;
      if (stateText) stateText.textContent = stateLabel;
    }

    function renderNews(body) {
      var items = Array.isArray(body) ? body
        : (body && Array.isArray(body.items)) ? body.items
        : (body && Array.isArray(body.news)) ? body.news
        : (body && Array.isArray(body.data)) ? body.data
        : null;

      if (!items || items.length === 0) {
        renderState("今天的资讯还没有到", "接口有了回应，但里面没有可显示的内容。", "ok", "接口正常");
        return;
      }

      news.innerHTML = items.slice(0, 12).map(function (item) {
        var title = item.title || item.name || "未命名";
        var text = item.text || item.summary || item.body || item.content || item.desc || "";
        var tag = item.tag || item.category || "资讯";
        var when = item.time || item.date || item.published_at || item.ts || "";
        var link = safeLink(item.link || item.url || "");
        var image = newsImage(item);

        // No image: a plain card, text only. With one: the picture leads at the
        // card's top, full width, then the text under it.
        return '<article class="news-card' + (image ? " news-card--media" : "") + '">' +
          (image
            ? '<div class="news-media"><img src="' + esc(image) +
              '" alt="" loading="lazy" referrerpolicy="no-referrer"></div>'
            : "") +
          '<div class="news-card-body">' +
          '<span class="news-tag">' + esc(tag) + "</span>" +
          '<h3 class="news-title">' + esc(title) + "</h3>" +
          (text ? '<p class="news-body">' + esc(text) + "</p>" : "") +
          "</div>" +
          (when || link
            ? '<div class="news-foot">' +
              (when ? '<span class="news-when">' + icon("clock", "icon--sm") + esc(when) + "</span>" : "") +
              (link
                ? '<a class="news-link" href="' + esc(link) +
                  '" target="_blank" rel="noreferrer noopener">阅读原文</a>'
                : "") +
              "</div>"
            : "") +
          "</article>";
      }).join("");
      if (state) state.dataset.state = "ok";
      if (stateText) stateText.textContent = items.length + " 条";
    }

    /**
     * The picture a news item carries, from whichever field the server uses.
     *
     * Only `http:` and `https:` are allowed through — `javascript:` in an `<img
     * src>` must never reach the page even though a modern browser would refuse to
     * run it. A relative path is deliberately *not* resolved here: the shell has
     * already made every image absolute against the publisher, so one that is still
     * relative is one this page cannot place, and completing it against 服务地址
     * would fetch a Mojang picture from the hongshi site.
     */
    function newsImage(item) {
      var value = item.image || item.img || item.cover || item.thumbnail || item.picture || "";
      if (typeof value === "object" && value) {
        value = value.url || value.src || "";
      }
      return safeLink(value);
    }

    /** An `http(s)` URL, or empty. Used for every `src` and `href` a card writes. */
    function safeLink(value) {
      value = String(value || "").trim();
      if (/^https?:\/\//i.test(value)) return value;
      if (value.startsWith("//")) return "https:" + value;
      return "";
    }

    /*
     * The news is Minecraft's own launcher feed, fetched by the shell and trimmed
     * there: the upstream file is 64 KB of a hundred entries and the shell hands
     * over the first few, with the pictures already made absolute. The page never
     * talks to Mojang itself — a `fetch` from `http://127.0.0.1:<port>` to another
     * origin is cross-origin, and `connect-src 'self'` says so.
     */
    S.apiJson("/api/daily-news").then(function (result) {
      var body = result.body || {};
      var where = body.source || "launchercontent.mojang.com";
      note.textContent = "来自 " + where + "（前 " + (body.limit || 3) + " 条）";

      if (body.state === "ok") {
        renderNews(body.data);
        return;
      }

      // A 200 the shell could not read is a different thing from a request that did
      // not complete, and neither is something the user did.
      if (body.state === "empty") {
        renderState("没读到任何一条资讯",
          "接口有回应，但里面没有能认出来的资讯，可能是官方换了格式。", "down", "格式变化");
        return;
      }

      renderState("没能取到资讯",
        "请求没有完成：" + esc(body.reason || "原因未知") +
        "。<br>可以在设置页检查代理设置。", "down", "取不到");
    }).catch(function (err) {
      renderState("没能取到资讯", "客户端没有响应：" + esc(String(err)), "down", "失败");
    });

    /*
     * Services, one card each.
     *
     * The tunnel is a service like the other two, not a separate part of the page:
     * 联机 is what it *is*, this is what it *costs you and where it stands*. So the
     * three cards share one shape — icon, name, state, value, detail — and the
     * tunnel's detail swaps between "nothing running" and its live address.
     */
    var servicesBox = el.querySelector("#usage");
    var unsubscribe = S.tunnel.subscribe(renderServices);

    /** A card that is not available yet, with the reason rather than just a mark. */
    function unavailableCard(iconName, name, value, note, lines) {
      return '<article class="usage-card" data-available="false">' +
        '<div class="usage-top">' + icon(iconName) +
        '<span class="usage-name">' + esc(name) + "</span>" +
        '<span class="usage-state">' + icon("cross", "icon--sm") + "不可用</span></div>" +
        '<div class="usage-value">' + esc(value) + "</div>" +
        '<div class="meter"><span style="width:0"></span></div>' +
        '<p class="usage-note">' + esc(note) + "</p>" +
        '<div class="usage-lines">' + lines.map(function (line) {
          return "<span>" + line + "</span>";
        }).join("") + "</div>" +
        "</article>";
    }

    function tunnelCard() {
      var tunnel = S.tunnel.tunnel;
      var age = S.tunnel.age();

      if (!tunnel) {
        return '<article class="usage-card" data-available="true">' +
          '<div class="usage-top">' + icon("link") +
          '<span class="usage-name">联机隧道</span>' +
          '<span class="usage-state">' + icon("cross", "icon--sm") + "未开启</span></div>" +
          '<div class="usage-value">当前没有隧道</div>' +
          '<p class="usage-note">开启后这里显示连接地址、创建时间与运行时长，日志在右侧面板。</p>' +
          '<div class="tunnel-actions">' +
          '<button type="button" class="btn btn--primary btn--small" id="home-start">' +
          icon("play", "icon--sm") + "开启隧道</button></div>" +
          "</article>";
      }

      return '<article class="usage-card usage-card--live" data-available="true">' +
        '<div class="usage-top">' + icon("link") +
        '<span class="usage-name">联机隧道</span>' +
        '<span class="usage-state usage-state--live">' + icon("check", "icon--sm") + "运行中</span></div>" +
        '<div class="addr" data-assigned="' + (tunnel.endpoint ? "true" : "false") + '">' +
        "<span>" + esc(tunnel.endpoint || "等待分配地址") + "</span>" +
        (tunnel.endpoint
          ? '<button type="button" class="addr-copy" id="home-copy" title="复制地址" aria-label="复制地址">' +
            icon("copy", "icon--sm") + "</button>"
          : "") +
        "</div>" +
        '<div class="usage-lines" style="margin-top:10px">' +
        "<span>创建于 <b id=\"home-created\">" + esc(age === null ? "—" : S.relativeTime(age)) + "</b></span>" +
        "<span>已运行 <b id=\"home-uptime\">" + esc(age === null ? "—" : S.duration(age)) + "</b></span>" +
        "<span>节点 <b>" + esc(tunnel.node || "自动") + "</b></span>" +
        "<span>方式 <b>" + esc(tunnel.mode || "中转 · TCP") + "</b></span>" +
        "</div>" +
        '<div class="tunnel-actions">' +
        '<button type="button" class="btn btn--ghost btn--small" id="home-logs">' +
        icon("info", "icon--sm") + "查看日志</button>" +
        '<button type="button" class="btn btn--danger btn--small" id="home-stop">' +
        icon("close", "icon--sm") + "关闭隧道</button>" +
        "</div>" +
        "</article>";
    }

    function renderServices() {
      if (!servicesBox) return;
      servicesBox.innerHTML =
        tunnelCard() +
        unavailableCard("disk", "云硬盘", "暂未接入红石云存档服务",
          "接入后这里显示已用容量与总容量，以及各存档的占用。",
          ["已用 <b>—</b>", "总量 <b>—</b>"]) +
        unavailableCard("server", "云服务器", "暂未接入红石租聘服服务",
          "接入后这里显示每台服务器的规格、地区与剩余时长。",
          ["在用机器 <b>—</b>", "剩余时长 <b>—</b>"]);

      var startButton = servicesBox.querySelector("#home-start");
      if (startButton) {
        startButton.addEventListener("click", function () { S.go("connect"); });
      }
      var copy = servicesBox.querySelector("#home-copy");
      if (copy) {
        copy.addEventListener("click", function () { S.copyText(S.tunnel.tunnel.endpoint, "连接地址"); });
      }
      var logs = servicesBox.querySelector("#home-logs");
      if (logs) logs.addEventListener("click", function () { S.drawerOpen(true); });
      var stop = servicesBox.querySelector("#home-stop");
      if (stop) {
        stop.addEventListener("click", function () {
          stop.disabled = true;
          stop.textContent = "正在关闭…";
          S.tunnel.stop().then(renderServices);
        });
      }
    }

    renderServices();

    function renderSession() {
      if (!sessionKv) return;
      var health = S.health;
      if (!health) {
        sessionKv.innerHTML = "<dt>状态</dt><dd>正在读取…</dd>";
        return;
      }
      var webSource = health.web_source === "embedded in the binary"
        ? "内嵌在二进制里"
        : esc(health.web_source);
      var base = (S.settings && S.settings.api_base) || "读取中…";
      sessionKv.innerHTML =
        "<dt>客户端版本</dt><dd>" + esc(health.shell) + " " + esc(health.version) + "</dd>" +
        "<dt>界面来源</dt><dd>" + webSource + "</dd>" +
        "<dt>服务地址</dt><dd>" + esc(base) + "</dd>" +
        "<dt>运行时长</dt><dd>" + esc(S.duration(health.uptime_seconds)) + "</dd>";
    }
    renderSession();

    // The board is on screen; fill in the settings-derived row once they arrive.
    // `loadSettings` answers from its cache when `boot()` already read them, which is
    // the usual case: this is only here for the run where it has not.
    S.loadSettings(false).then(function () { renderSession(); });

    return {
      title: "主页",
      onHealth: renderSession,
      onTick: function () {
        renderSession();
        // Only the two counters are rewritten. Re-rendering the card every second
        // would replace its buttons, and a button mid-click — or holding keyboard
        // focus — does not survive being replaced.
        var created = servicesBox ? servicesBox.querySelector("#home-created") : null;
        var uptime = servicesBox ? servicesBox.querySelector("#home-uptime") : null;
        var age = S.tunnel.age();
        if (age !== null && created && uptime) {
          created.textContent = S.relativeTime(age);
          uptime.textContent = S.duration(age);
        }
      },
      destroy: function () {
        unsubscribe();
      }
    };
  }

  /* --------------------------------------------------------------- connect */

  /** TCP-over-UDP is a real mode; the kernel only does relayed TCP today. */
  var MODES = [
    { id: "relay-tcp", kind: "relay", label: "中转 · TCP", available: true },
    { id: "relay-udp", kind: "relay", label: "中转 · UDP", available: false, why: "内核还没有实现 UDP 转发" },
    { id: "p2p-tcp", kind: "p2p", label: "P2P · TCP", available: false, why: "内核还没有实现打洞" },
    { id: "p2p-udp", kind: "p2p", label: "P2P · UDP", available: false, why: "TCP-over-UDP 打洞还没有实现" }
  ];

  function connectPage(el) {
    var selectedMode = "relay-tcp";
    var selectedNode = "auto";
    var ticker = null;

    el.innerHTML =
      pageHead("联机", "把本机的游戏端口，通过中转服务器交给朋友。玩家在游戏里填地址即可，不需要装任何东西。") +
      '<div class="connect-grid">' +
      '<div class="connect-form">' +
      section("本机端口与转发方式", "link",
        '<div class="field-row">' +
        '<div class="field"><label class="label" for="game-port">本地游戏端口</label>' +
        '<input class="input input--mono" id="game-port" type="number" inputmode="numeric" ' +
        'min="1" max="65535" value="25565" autocomplete="off"></div>' +
        '<div class="field"><label class="label" for="game-host">本机地址</label>' +
        '<input class="input input--mono" id="game-host" type="text" value="127.0.0.1" autocomplete="off"></div>' +
        "</div>" +
        '<div class="mode-groups" style="margin-top:16px">' +
        '<div class="mode-group"><span class="label">转发方式</span>' +
        '<div class="segmented" id="mode-kind">' +
        '<label data-kind="relay"><input type="radio" name="kind" value="relay" checked><span>中转</span></label>' +
        '<label data-kind="p2p"><input type="radio" name="kind" value="p2p"><span>P2P</span></label>' +
        "</div>" +
        '<p class="mode-hint" id="kind-hint"></p></div>' +
        '<div class="mode-group"><span class="label">协议</span>' +
        '<div class="segmented" id="mode-proto"></div>' +
        '<p class="mode-hint" id="proto-hint"></p></div>' +
        "</div>") +
      section("中转服务器", "globe",
        '<div class="node-box" id="nodes"></div>' +
        '<div style="display:flex;gap:10px;margin-top:12px;flex-wrap:wrap">' +
        '<button type="button" class="btn btn--ghost" id="probe">' + icon("refresh", "icon--sm") + "重新测速</button>" +
        '<button type="button" class="btn btn--ghost" id="reload-nodes">' + icon("globe", "icon--sm") + "刷新节点列表</button>" +
        "</div>" +
        '<p class="probe-note" id="probe-note"></p>',
        "延迟由客户端直接连接节点的控制端口测得，反映的是「你的网络到节点」这一段。" +
        "选「自动」时，会用探测到延迟最低的那个节点。") +
      "</div>" +
      '<div class="connect-side" id="connect-side"></div>' +
      "</div>";

    var portInput = el.querySelector("#game-port");
    var hostInput = el.querySelector("#game-host");
    var kindHint = el.querySelector("#kind-hint");
    var protoHint = el.querySelector("#proto-hint");
    var protoBox = el.querySelector("#mode-proto");
    var nodesBox = el.querySelector("#nodes");
    var probeNote = el.querySelector("#probe-note");
    var side = el.querySelector("#connect-side");

    S.loadSettings(false).then(function (loaded) {
      if (loaded && loaded.default_game_port) portInput.value = String(loaded.default_game_port);
    });

    /* ---- mode picker: two levels, only relay+TCP is live today ---- */

    function currentKind() {
      var checked = el.querySelector('input[name="kind"]:checked');
      return checked ? checked.value : "relay";
    }

    function renderProtocols() {
      var kind = currentKind();
      var forKind = MODES.filter(function (mode) { return mode.kind === kind; });
      protoBox.innerHTML = forKind.map(function (mode) {
        return '<label data-disabled="' + (mode.available ? "false" : "true") + '" title="' +
          esc(mode.why || "") + '">' +
          '<input type="radio" name="proto" value="' + mode.id + '"' +
          (mode.id === selectedMode ? " checked" : "") +
          (mode.available ? "" : " disabled") +
          "><span>" + esc(mode.label.split(" · ")[1]) + "</span></label>";
      }).join("");

      kindHint.textContent = kind === "relay"
        ? "经过中转服务器转发，兼容性最好，玩家侧零安装。"
        : "尝试在两端之间直接建立连接，不经过中转，延迟更低。";

      var mode = MODES.filter(function (m) { return m.id === selectedMode; })[0];
      protoHint.textContent = mode && !mode.available
        ? mode.why + "（当前只有「中转 · TCP」可用）"
        : "TCP-over-UDP 走同一套隧道，播放器侧无感。";
    }

    /* Both listeners below are delegated on `el`, which is the *persistent* `.page`
       element — the router only clears its children between navigations. A listener
       put there outlives the page unless `destroy` takes it off again, and every
       visit would add another one: one click on 开启隧道 would then fire one
       `startTunnel()` per visit so far, and the second and later ones are refused
       with "已经有一个隧道在运行了" by a tunnel the first one had just started. */
    function onModeChange(event) {
      if (event.target.name === "kind") {
        // Switching kind selects that kind's first protocol; only relay+TCP exists.
        var kind = currentKind();
        selectedMode = kind === "relay" ? "relay-tcp" : "p2p-tcp";
        renderProtocols();
      } else if (event.target.name === "proto") {
        selectedMode = event.target.value;
        renderProtocols();
      }
    }

    el.addEventListener("change", onModeChange);
    renderProtocols();

    /* ---- node list ---- */

    /*
     * The relay picker: a button that opens a list of rows.
     *
     * It was a native `<select>` first, and the honest reason it is not one any more
     * is that a `<select>` cannot be styled: on Windows the popup is drawn by the OS,
     * so `border-radius`, `padding` and layout on an `<option>` are ignored and every
     * row came out as one flat string with the name, the latency and the state run
     * together — "南京 · 37 ms · 可建隧道". This is a listbox instead, and both halves
     * of it are drawn from `pickerRows()`, so the button and the open list cannot
     * disagree about what exists.
     */
    var store = S.nodes;
    var pickerOpen = false;
    var activeIndex = 0;
    var triggerEl = null;
    var listEl = null;

    /** How a node's last measurement is described, in one key. */
    function nodeState(node) {
      return node.probe_state || (node.reachable === false ? "dead" : S.latencyState(node.latency_ms));
    }

    function stateLabel(state) {
      return S.NODE_STATE[state] || S.LATENCY_LABEL[state] || "";
    }

    /**
     * Every choice, as data: the row the button shows and the rows the list shows.
     *
     * `自动选择` carries no hostname — its second line is the promise it makes — and a
     * node carries its own plus whatever the probe said about why it is unusable.
     */
    function pickerRows() {
      var best = store.best();
      var rows = [{
        value: "auto",
        name: "自动选择",
        host: "",
        note: "挑延迟最低的可用节点",
        ms: best ? best.latency_ms : null,
        state: best ? nodeState(best) : "unknown"
      }];
      store.nodes.forEach(function (node) {
        rows.push({
          value: node.host,
          name: node.region || node.host,
          host: node.host,
          note: node.probe_reason || "",
          ms: node.latency_ms,
          state: nodeState(node)
        });
      });
      return rows;
    }

    function rowByValue(value) {
      var rows = pickerRows();
      for (var i = 0; i < rows.length; i++) {
        if (rows[i].value === value) return rows[i];
      }
      return null;
    }

    /** One row: the node on the left, its latency and meaning on the right. */
    function rowHtml(row) {
      return '<span class="picker-text">' +
        '<span class="picker-name">' + esc(row.name) + "</span>" +
        (row.host ? '<span class="picker-host">' + esc(row.host) + "</span>" : "") +
        (row.note ? '<span class="picker-note">' + esc(row.note) + "</span>" : "") +
        "</span>" +
        '<span class="picker-meta"><span class="dot" data-latency="' + esc(row.state) + '"></span>' +
        "<b>" + esc(S.latencyText(row.ms)) + "</b>" +
        '<span class="picker-state">' + esc(stateLabel(row.state)) + "</span></span>";
    }

    function renderTrigger() {
      if (!triggerEl) return;
      triggerEl.innerHTML = rowHtml(rowByValue(selectedNode) || pickerRows()[0]) +
        '<span class="picker-caret" aria-hidden="true"></span>';
    }

    function renderList() {
      if (!listEl) return;
      var rows = pickerRows();
      listEl.innerHTML = rows.map(function (row, index) {
        return '<li class="picker-option" id="node-option-' + index + '" role="option" ' +
          'data-value="' + esc(row.value) + '" data-index="' + index + '" ' +
          'aria-selected="' + (row.value === selectedNode ? "true" : "false") + '">' +
          '<span class="picker-mark">' + icon("check", "icon--sm") + "</span>" +
          rowHtml(row) +
          "</li>";
      }).join("");
    }

    /** The keyboard cursor, which is not the same thing as the chosen row. */
    function applyActive() {
      if (!listEl) return;
      var options = listEl.querySelectorAll(".picker-option");
      for (var i = 0; i < options.length; i++) {
        if (i === activeIndex) options[i].setAttribute("data-active", "true");
        else options[i].removeAttribute("data-active");
      }
      if (options[activeIndex]) {
        triggerEl.setAttribute("aria-activedescendant", options[activeIndex].id);
      }
    }

    function onDocumentClick(event) {
      // A click on the button or the list is this control's own business; anything
      // else means the user has moved on.
      if (event.target && event.target.closest && event.target.closest("#node-picker")) return;
      closePicker();
    }

    function openPicker() {
      if (pickerOpen || !listEl) return;
      pickerOpen = true;
      listEl.hidden = false;
      triggerEl.setAttribute("aria-expanded", "true");
      nodesBox.querySelector("#node-picker").setAttribute("data-open", "true");

      var rows = pickerRows();
      activeIndex = 0;
      for (var i = 0; i < rows.length; i++) {
        if (rows[i].value === selectedNode) activeIndex = i;
      }
      applyActive();

      // `remove` first: re-opening (a probe can rebuild this control while it is
      // open) must not leave a second listener behind on the document.
      document.removeEventListener("click", onDocumentClick);
      document.addEventListener("click", onDocumentClick);
    }

    function closePicker() {
      document.removeEventListener("click", onDocumentClick);
      if (!pickerOpen) return;
      pickerOpen = false;
      if (listEl) listEl.hidden = true;
      if (triggerEl) {
        triggerEl.setAttribute("aria-expanded", "false");
        triggerEl.removeAttribute("aria-activedescendant");
      }
      var picker = nodesBox.querySelector("#node-picker");
      if (picker) picker.removeAttribute("data-open");
    }

    function choose(value) {
      if (rowByValue(value)) selectedNode = value;
      closePicker();
      renderNodes();
      // `renderNodes` rebuilt the trigger, so focus the one that is on screen now.
      if (triggerEl) triggerEl.focus();
    }

    function moveActive(step) {
      var count = pickerRows().length;
      activeIndex = (activeIndex + step + count) % count;
      applyActive();
    }

    function onTriggerKey(event) {
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        if (!pickerOpen) openPicker();
        else moveActive(event.key === "ArrowDown" ? 1 : -1);
        return;
      }
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        if (!pickerOpen) openPicker();
        else {
          var row = pickerRows()[activeIndex];
          if (row) choose(row.value);
        }
        return;
      }
      if (event.key === "Escape" && pickerOpen) {
        event.preventDefault();
        closePicker();
      }
    }

    /** Draw whatever the store currently holds, including why it is empty. */
    function renderNodes() {
      var wasOpen = pickerOpen;

      if (!store.nodes.length) {
        triggerEl = null;
        listEl = null;
        if (store.state === "loading" || store.state === "idle") {
          nodesBox.innerHTML = '<div class="empty">正在读取节点列表…</div>';
        } else if (store.state === "missing") {
          nodesBox.innerHTML = emptyBox("官方节点接口还没有上线。可以在设置页检查服务地址与代理。");
        } else {
          nodesBox.innerHTML = emptyBox(
            "取不到节点列表：" + (store.reason || "原因未知") + "。可以在设置页检查服务地址与代理。");
        }
        probeNote.textContent = "";
        renderTunnel();
        return;
      }

      nodesBox.innerHTML =
        '<div class="picker" id="node-picker">' +
        '<button type="button" class="picker-trigger" id="node-trigger" role="combobox" ' +
        'aria-haspopup="listbox" aria-expanded="false" aria-controls="node-list" ' +
        'aria-label="中转服务器"></button>' +
        '<ul class="picker-list" id="node-list" role="listbox" aria-label="中转服务器" hidden></ul>' +
        "</div>";

      triggerEl = nodesBox.querySelector("#node-trigger");
      listEl = nodesBox.querySelector("#node-list");

      // A selection pointing at a node that is no longer listed falls back rather
      // than leaving the button blank — a refresh can drop a machine.
      if (!rowByValue(selectedNode)) selectedNode = "auto";
      renderTrigger();
      renderList();

      triggerEl.addEventListener("click", function () {
        if (pickerOpen) closePicker();
        else openPicker();
      });
      triggerEl.addEventListener("keydown", onTriggerKey);
      listEl.addEventListener("click", function (event) {
        var option = event.target.closest ? event.target.closest(".picker-option") : null;
        if (option) choose(option.dataset.value);
      });

      if (wasOpen) openPicker();

      var best = store.best();
      if (best && store.probedAt) {
        probeNote.textContent = "最近一次测速：" + S.relativeTime((Date.now() - store.probedAt) / 1000) +
          "，最快可用节点是 " + (best.region || best.host) + "（" + Math.round(best.latency_ms) + " ms）。" +
          " 这一份结果在客户端打开时读取一次，切换页面不会重新测速。";
      } else {
        probeNote.textContent = "";
      }
      renderTunnel();
    }

    // Redrawn from the store whenever it changes — a refresh or a probe started
    // anywhere, including one still running from startup.
    var unsubscribeNodes = store.subscribe(function () { renderNodes(); });

    el.querySelector("#probe").addEventListener("click", function () {
      store.probe(true).then(function () {
        var best = store.best();
        if (!best) return;
        S.termWrite("最快可用节点：" + (best.region || best.host) + "（" + best.host + "） " +
          Math.round(best.latency_ms) + " ms", "ok");
        // Say which nodes answered a ping but have no tunnel port: that is the
        // difference between "your network is broken" and "that node is not accepting
        // tunnels", and the user can act on only one of them.
        var blocked = store.nodes.filter(function (node) { return node.probe_state === "ping"; });
        if (blocked.length) {
          S.termWrite("有 " + blocked.length + " 个节点能 ping 通但控制端口没开，无法建隧道：" +
            blocked.map(function (n) { return n.region; }).join("、"), "warn");
        }
      });
    });

    el.querySelector("#reload-nodes").addEventListener("click", function () {
      S.termWrite("重新读取节点列表", "dim");
      store.load(true).then(function (nodes) {
        if (nodes.length) store.probe(false);
      });
    });

    /* ---- tunnel ---- */

    function chosenNode() {
      if (selectedNode === "auto") return store.best();
      return store.nodes.filter(function (n) { return n.host === selectedNode; })[0] || null;
    }

    function renderTunnel() {
      if (!side) return;
      var tunnel = S.tunnel.tunnel;
      var createdAt = S.tunnel.createdAt;
      var kernel = S.kernel.info;

      if (!tunnel) {
        var node = chosenNode();
        // The button's state follows the kernel, not the settings: a client with no
        // kernel cannot start a tunnel however correct the form is, and the way out
        // of that is the download, not a disabled button with no explanation.
        var found = !!(kernel && kernel.found);
        var exited = kernel && kernel.state === "exited" && kernel.exit_meaning;

        side.innerHTML =
          '<div class="card">' +
          '<div class="card-head"><h3>隧道</h3>' +
          pill(null, exited ? "down" : "idle", exited ? "已结束" : "未开启") +
          "</div>" +
          (exited
            ? '<p class="usage-note" style="margin-top:12px">' + esc(kernel.exit_meaning) +
              (kernel.exit_code === null || kernel.exit_code === undefined
                ? "" : "（退出代码 " + esc(String(kernel.exit_code)) + "）") + "</p>"
            : '<p class="usage-note" style="margin-top:12px">' +
              "开启后会在这里显示连接地址与创建时间，右侧同时打开日志。</p>") +
          '<dl class="kv" style="margin-top:12px">' +
          "<dt>转发方式</dt><dd>" + esc(modeLabel(selectedMode)) + "</dd>" +
          "<dt>本机端口</dt><dd>" + esc(portInput.value || "—") + "</dd>" +
          "<dt>目标节点</dt><dd>" +
          esc(selectedNode === "auto"
            ? (node ? node.host + "（自动）" : "自动（暂无可用节点）")
            : selectedNode) + "</dd>" +
          (found ? "" : "<dt>内核</dt><dd>" + esc(kernelExpected()) + "</dd>") +
          "</dl>" +
          '<div style="margin-top:16px;display:flex;gap:10px;flex-wrap:wrap">' +
          '<button type="button" class="btn btn--primary" id="tunnel-start"' +
          (found ? "" : " disabled") + ">" +
          icon("play", "icon--sm") + "开启隧道</button>" +
          (found ? "" :
            '<button type="button" class="btn btn--ghost" id="kernel-download">' +
            icon("download", "icon--sm") + "下载内核</button>") +
          "</div>" +
          (found ? "" : kernelNotice(kernel)) +
          "</div>";
        return;
      }

      var elapsed = createdAt ? Math.floor((Date.now() - createdAt) / 1000) : 0;
      side.innerHTML =
        '<div class="card">' +
        '<div class="card-head"><h3>隧道</h3>' +
        '<span class="pill" data-state="ok"><span class="pill-dot"></span>运行中</span></div>' +
        '<div style="margin-top:14px">' +
        '<div class="addr" data-assigned="' + (tunnel.endpoint ? "true" : "false") + '">' +
        "<span>" + esc(tunnel.endpoint || "还没有分配地址") + "</span>" +
        (tunnel.endpoint
          ? '<button type="button" class="addr-copy" id="copy-addr" title="复制地址" aria-label="复制地址">' +
            icon("copy", "icon--sm") + "</button>"
          : "") +
        "</div></div>" +
        '<dl class="tunnel-grid" style="margin-top:14px">' +
        cell("创建于", createdAt ? S.relativeTime(elapsed) : "—") +
        cell("已运行", S.duration(elapsed)) +
        cell("转发方式", modeLabel(selectedMode)) +
        cell("本机端口", String(portInput.value || "—")) +
        cell("节点", tunnel.node || "—") +
        cell("隧道号", tunnel.uuid || "—") +
        "</dl>" +
        '<div style="margin-top:16px;display:flex;gap:10px;flex-wrap:wrap">' +
        '<button type="button" class="btn btn--danger" id="tunnel-stop">' +
        icon("close", "icon--sm") + "关闭隧道</button>" +
        '<button type="button" class="btn btn--ghost" id="tunnel-log">' +
        icon("info", "icon--sm") + "查看日志</button>" +
        "</div></div>";

      var copy = side.querySelector("#copy-addr");
      if (copy) {
        copy.addEventListener("click", function () { S.copyText(tunnel.endpoint, "连接地址"); });
      }
      side.querySelector("#tunnel-stop").addEventListener("click", stopTunnel);
      side.querySelector("#tunnel-log").addEventListener("click", function () {
        S.drawerOpen(true);
      });
    }

    function cell(term, value) {
      return '<div class="tunnel-cell"><dt>' + esc(term) + "</dt><dd>" +
        "<code>" + esc(value) + "</code></dd></div>";
    }

    /** The file name this build would download, e.g. `hongshic-windows-amd64.exe`. */
    function kernelExpected() {
      var kernel = S.kernel.info;
      return (kernel && kernel.expected_file) || "hongshic";
    }

    /** What to do about a missing kernel: the path, and the button above. */
    function kernelNotice(kernel) {
      var dir = (kernel && kernel.core_dir) || "core";
      var platform = kernel && kernel.platform ? kernel.platform + "-" + kernel.arch : "当前平台";
      return notice("warn",
        "<strong>没有找到内核 <code>" + esc(kernelExpected()) + "</code>。</strong>" +
        "隧道由 <code>hongshic</code> 承担，客户端只负责把它跑起来。<br>" +
        "点上面的「下载内核」会自动取匹配当前平台的版本（<code>" + esc(platform) + "</code>），" +
        "下载好之后就能直接开隧道。<br>" +
        "也可以自己把内核放进客户端旁边的 <code>core</code> 目录（也就是 <code>" +
        esc(dir) + "</code>），然后重开客户端。");
    }

    function modeLabel(id) {
      var mode = MODES.filter(function (m) { return m.id === id; })[0];
      return mode ? mode.label : id;
    }

    function startTunnel() {
      var port = parseInt(portInput.value, 10);
      if (!(port > 0 && port < 65536)) {
        S.toast("本地端口要填 1 到 65535 之间的数字", "warn");
        portInput.focus();
        return;
      }
      var node = chosenNode();
      if (selectedNode !== "auto" && !node) {
        S.toast("选中的节点不在列表里，请重新选择", "warn");
        return;
      }
      if (!node) {
        S.toast("还没有可用的中转节点，先刷新一下节点列表", "warn");
        return;
      }

      S.drawerOpen(true);
      S.termWrite("准备开启隧道", "ident");
      S.termWrite("转发方式 " + modeLabel(selectedMode) + " · 本机 " +
        hostInput.value + ":" + port + " · 节点 " + node.host, "dim");

      S.apiSend("/api/tunnel/start", {
        mode: selectedMode,
        relay: node.host,
        game_host: hostInput.value,
        game_port: port
      }).then(function (result) {
        var body = result.body || {};

        // The kernel is a child process: starting it succeeds long before it has a
        // tunnel. The endpoint arrives on its stdout a moment later and the poll in
        // `S.kernel` is what lifts it into the card, so "started" is all that is
        // claimed here.
        if (result.ok && body.state === "ok") {
          S.termWrite("内核已启动，等待分配隧道地址…", "ok");
          S.kernel.watch(true);
          S.kernel.drain();
          return;
        }

        S.termWrite(body.reason || "隧道没能开启", "warn");
        S.toast(body.reason || "隧道没能开启", "warn", 6000);
        if (body.kernel) S.kernel.refresh();
      }).catch(function (err) {
        S.termWrite("请求失败：" + String(err), "error");
        S.toast("请求失败：" + String(err), "warn");
      });
    }

    function downloadKernel(button) {
      var label = button.textContent;
      button.disabled = true;
      button.textContent = "正在下载…";
      S.drawerOpen(true);
      S.termWrite("正在从官方站点下载内核（" + kernelExpected() + "）…", "ident");

      S.kernel.download().then(function (result) {
        var body = result.body || {};
        if (result.ok) {
          S.termWrite("内核已安装：" + (body.path || "") + "（" +
            Math.round((body.bytes || 0) / 1024) + " KB）", "ok");
          S.toast("内核已就绪，可以开启隧道了");
          return;
        }
        S.termWrite("下载内核失败：" + (body.reason || "原因未知"), "warn");
        S.toast("下载内核失败：" + (body.reason || "原因未知"), "warn", 8000);
      }).catch(function (err) {
        S.termWrite("下载内核失败：" + String(err), "error");
        S.toast("下载内核失败：" + String(err), "warn", 8000);
      }).then(function () {
        if (button.isConnected) {
          button.disabled = false;
          button.textContent = label;
        }
      });
    }

    function stopTunnel() {
      S.termWrite("正在关闭隧道…", "dim");
      S.tunnel.stop();
    }

    function onPageClick(event) {
      if (!event.target.closest) return;
      if (event.target.closest("#tunnel-start")) startTunnel();
      var download = event.target.closest("#kernel-download");
      if (download) downloadKernel(download);
    }

    el.addEventListener("click", onPageClick);

    renderNodes();

    // Redraw when the tunnel changes — including when the 主页 page stops it — when the
    // node store changes (a probe still running from startup), and when the kernel
    // does, which is what makes the button appear once a download lands.
    var unsubscribe = S.tunnel.subscribe(renderTunnel);
    var unsubscribeKernel = S.kernel.subscribe(renderTunnel);

    S.termWrite("联机页面已就绪，当前只有「中转 · TCP」可用", "dim");

    return {
      title: "联机",
      destroy: function () {
        /* `el` is the same element for the life of the tab, so the two delegated
           listeners above have to come off by name. `unsubscribe` is not enough:
           a store subscription is a function the store drops, while a DOM listener
           stays on the node until it is removed. */
        el.removeEventListener("click", onPageClick);
        el.removeEventListener("change", onModeChange);
        // The relay picker's "click outside closes it" listener lives on the
        // document, which outlives this page — leaving it behind would keep closing
        // a list nobody can open any more, once per visit.
        closePicker();
        unsubscribe();
        unsubscribeKernel();
        unsubscribeNodes();
        if (ticker) window.clearInterval(ticker);
      },
      onTick: function () {
        var cells = side ? side.querySelectorAll(".tunnel-cell code") : null;
        var age = S.tunnel.age();
        if (S.tunnel.running() && age !== null && cells && cells.length >= 2) {
          cells[0].textContent = S.relativeTime(age);
          cells[1].textContent = S.duration(age);
        }
      }
    };
  }

  /* -------------------------------------------------------------- settings */

  function settingsPage(el) {
    el.innerHTML =
      pageHead("设置", "客户端本机的一些开关。它们保存在客户端旁边的一个 JSON 文件里，不会上传。") +
      section("服务地址", "globe",
        '<div class="settings-form">' +
        '<div class="field"><label class="label" for="set-base">官方站点地址</label>' +
        '<input class="input input--mono" id="set-base" type="text" autocomplete="off" ' +
        'placeholder="https://hongshi.site"></div>' +
        '<p class="section-note" style="margin:0">节点列表与每日资讯都从这里取。' +
        "页面本身不能直接请求它（跨域会被浏览器拦下），是客户端代取的。</p>" +
        "</div>") +
      section("网络", "link",
        '<div class="settings-form">' +
        '<div class="field"><label class="label" for="set-proxy">代理地址</label>' +
        '<input class="input input--mono" id="set-proxy" type="text" autocomplete="off" ' +
        'placeholder="留空表示自动，例如 http://127.0.0.1:7897"></div>' +
        '<label class="switch"><input type="checkbox" id="set-sysproxy">' +
        "<span>没有填代理时，使用系统的代理设置</span></label>" +
        '<div class="field"><label class="label" for="set-cache">节点列表缓存时长（秒）</label>' +
        '<input class="input input--mono" id="set-cache" type="number" min="30" max="86400" autocomplete="off"></div>' +
        "</div>",
        "取一次节点列表就够用一会儿，缓存可以少打扰官方服务器，也让页面打开更快。") +
      section("默认值", "link",
        '<div class="settings-form">' +
        '<div class="field"><label class="label" for="set-port">联机页默认的本地游戏端口</label>' +
        '<input class="input input--mono" id="set-port" type="number" min="1" max="65535" autocomplete="off"></div>' +
        "</div>") +
      section("保存", "save",
        '<div class="settings-actions">' +
        '<button type="button" class="btn btn--primary" id="set-save">' + icon("save", "icon--sm") + "保存设置</button>" +
        '<button type="button" class="btn btn--ghost" id="set-reload">' + icon("refresh", "icon--sm") + "放弃修改</button>" +
        pill("set-state", "down", "未保存") +
        "</div>" +
        '<dl class="kv" style="margin-top:14px" id="set-where"></dl>') +
      section("关于", "info",
        '<div class="card">' +
        '<div class="about-row">' +
        '<div><div class="about-name">红石联机客户端</div>' +
        '<div class="about-version" id="about-version">读取中…</div></div>' +
        pill("about-state", "down", "未检查") +
        "</div>" +
        '<p class="usage-note" id="about-note">检查更新会向官方站点查询最新版本。</p>' +
        '<div class="tunnel-actions">' +
        '<button type="button" class="btn btn--ghost btn--small" id="about-check">' +
        icon("refresh", "icon--sm") + "检查更新</button>" +
        '<button type="button" class="btn btn--primary btn--small" id="about-download" hidden>' +
        icon("download", "icon--sm") + "下载新版本</button>" +
        "</div></div>",
        "新版本会发布在官方站点的下载页，接口与页面用的是同一份产物。");

    var base = el.querySelector("#set-base");
    var proxy = el.querySelector("#set-proxy");
    var sysProxy = el.querySelector("#set-sysproxy");
    var cache = el.querySelector("#set-cache");
    var port = el.querySelector("#set-port");
    var state = el.querySelector("#set-state");
    var where = el.querySelector("#set-where");
    var aboutState = el.querySelector("#about-state");
    var aboutNote = el.querySelector("#about-note");
    var aboutVersion = el.querySelector("#about-version");
    var aboutDownload = el.querySelector("#about-download");

    // The running build, from the client itself rather than from a constant here.
    S.apiJson("/api/version").then(function (result) {
      aboutVersion.textContent = versionLabel(result && result.body);
    });

    /** Which build this browser would need, in the download endpoint's vocabulary. */
    function platformQuery() {
      var ua = (navigator.userAgent || "").toLowerCase();
      var platform = (navigator.platform || "").toLowerCase();

      var os = "windows";
      if (ua.indexOf("mac") >= 0 || platform.indexOf("mac") >= 0) os = "macos";
      else if (ua.indexOf("linux") >= 0 || platform.indexOf("linux") >= 0) os = "linux";

      // `arm64` for Apple Silicon and ARM Linux; everything else here is x64.
      var arch = /arm64|aarch64/.test(ua) || /arm/.test(platform) ? "arm64" : "amd64";
      return "?kind=webui&platform=" + os + "&arch=" + arch;
    }

    el.querySelector("#about-check").addEventListener("click", function () {
      var button = el.querySelector("#about-check");
      button.disabled = true;
      setPill(aboutState, "busy", "检查中…");
      aboutNote.textContent = "正在向官方站点查询最新版本…";

      S.apiJson("/api/version").then(function (result) {
        button.disabled = false;
        var body = (result && result.body) || {};
        if (body.current) aboutVersion.textContent = versionLabel(body);

        if (body.update === true) {
          setPill(aboutState, "ok", "有新版本");
          aboutNote.textContent = "官方最新版本 v" + body.remote + "，当前 v" + body.current + "。";
          aboutDownload.hidden = false;
          return;
        }
        if (body.update === false) {
          setPill(aboutState, "ok", "已是最新");
          aboutNote.textContent = "当前 v" + body.current + " 就是官方最新版本。";
          aboutDownload.hidden = true;
          return;
        }

        /*
         * The third answer, and the honest one: the check did not happen. The
         * version endpoint does not exist yet, so this is the normal result today —
         * saying "已是最新" here would be a claim nobody made.
         */
        setPill(aboutState, "down", "无法检查");
        aboutNote.textContent = body.state === "missing"
          ? "官方站点还没有版本查询接口（/api/webui/version），所以暂时无法检查更新。"
          : "检查没有完成：" + (body.reason || "官方站点没有响应") + "。可以在上面调整服务地址与代理。";
        aboutDownload.hidden = true;
      }).catch(function (err) {
        button.disabled = false;
        setPill(aboutState, "down", "无法检查");
        aboutNote.textContent = "客户端没有响应：" + String(err);
      });
    });

    aboutDownload.addEventListener("click", function () {
      // Handed to the browser as a normal download, which is also what the site's
      // own download page links to.
      var url = (S.settings && S.settings.api_base ? S.settings.api_base.replace(/\/+$/, "") : "") +
        "/api/download/webui" + platformQuery();
      window.open(url, "_blank", "noopener");
    });

    function fill(loaded) {
      if (!loaded) return;
      base.value = loaded.api_base || "";
      proxy.value = loaded.proxy || "";
      sysProxy.checked = loaded.use_system_proxy !== false;
      cache.value = String(loaded.node_cache_seconds || 300);
      port.value = String(loaded.default_game_port || 25565);
      where.innerHTML =
        "<dt>配置文件</dt><dd>" + esc(S.settingsMeta.path || "—") + "</dd>" +
        "<dt>平台 HTTP</dt><dd>" + esc(S.settingsMeta.http_backend || "—") + "</dd>";
    }

    S.loadSettings(false).then(fill);

    function markDirty() {
      setPill(state, "busy", "有未保存的修改");
    }

    [base, proxy, sysProxy, cache, port].forEach(function (input) {
      input.addEventListener("input", markDirty);
      input.addEventListener("change", markDirty);
    });

    el.querySelector("#set-save").addEventListener("click", function () {
      var patch = {
        api_base: base.value.trim(),
        proxy: proxy.value.trim(),
        use_system_proxy: sysProxy.checked,
        node_cache_seconds: parseInt(cache.value, 10) || 300,
        default_game_port: parseInt(port.value, 10) || 25565
      };
      S.saveSettings(patch).then(function (result) {
        var body = result.body || {};
        if (result.ok && body.settings) {
          fill(body.settings);
          setPill(state, "ok", "已保存");
          S.toast("设置已保存到 " + (body.settings.path || "配置文件"));
        } else {
          setPill(state, "down", "保存失败");
          S.toast("保存失败：" + (body.error || "客户端没有响应"), "warn");
        }
      }).catch(function (err) {
        setPill(state, "down", "保存失败");
        S.toast("保存失败：" + String(err), "warn");
      });
    });

    el.querySelector("#set-reload").addEventListener("click", function () {
      S.loadSettings(true).then(function (loaded) {
        fill(loaded);
        setPill(state, "down", "未保存");
      });
    });

    return {
      title: "设置",
      onHealth: function () { /* the settings page shows no live shell state */ }
    };
  }

  /* ---------------------------------------------------------------- register */

  S.register("home", homePage);
  S.register("connect", connectPage);
  S.register("settings", settingsPage);
  S.register("soon", soonPage);

  // The routes exist now, so the shell can draw one. `app.js` owns the boot
  // sequence and calls this; without it nothing would ever render, which is why
  // the shell also has a watchdog for this script not arriving at all.
  S.ready();
})();
