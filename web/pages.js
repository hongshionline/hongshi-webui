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

  /* The relay store, held here rather than inside a page.
   *
   * It is a shell-level singleton — read once at startup, re-probed on demand — and
   * more than one page needs the same rows: 联机 builds its picker from them, and
   * `pickerRows()` below is the single place that decides what a row looks like.
   * Declaring it per page would give two pages two ideas of what a node is called. */
  var store = S.nodes;

  /*
   * What the 个性化 section's file dialog offers, and the size it warns about.
   *
   * Both are **hints**, and the shell is the authority: what a file really is gets
   * decided by its first bytes (`background.rs`), because a `.jpg` that is really an
   * animated GIF passes any name-based check. The 20 MB here saves a round trip; the
   * 20 MB there is what is enforced.
   */
  var BACKGROUND_ACCEPT = "image/jpeg,image/png,image/webp,image/bmp,image/avif";
  var BACKGROUND_MAX_MB = 20;

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

  /* ------------------------------------------------------ relay list, shared */

  /**
   * Which of the four readings a node's probe produced.
   *
   * `probe_state` is the shell's own verdict and the only field that distinguishes
   * 可建隧道 from 仅能 ping 通; `reachable` is the older, blunter one, and the latency
   * ladder is the fallback for a node the probe has not answered for yet.
   */
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
   * node carries its own plus whatever the probe said about why it is unusable. Both
   * 联机's picker and its trigger read from here, which is what keeps the closed button
   * and the open list from disagreeing about what exists.
   */
  function pickerRows() {
    var best = store.best();
    var rows = [{
      value: "auto",
      name: "自动选择",
      host: "",
      note: "挑延迟最低的可用节点",
      ms: best ? best.latency_ms : null,
      state: best ? nodeState(best) : "unknown",
    }];
    store.nodes.forEach(function (node) {
      rows.push({
        value: node.host,
        name: node.region || node.host,
        host: node.host,
        note: node.probe_reason || "",
        ms: node.latency_ms,
        state: nodeState(node),
      });
    });
    return rows;
  }

  /** The filename the client is looking for, straight from the kernel's own report. */
  function kernelExpected() {
    var kernel = S.kernel.info;
    if (kernel && kernel.expected_file) return kernel.expected_file;
    return "hongshic";
  }

  /**
   * What to tell the user when the kernel is not there — and what to let them do.
   *
   * The client cannot open a room without `hongshic`, and downloading one is a real
   * decision, so the notice carries the button rather than only reporting the absence.
   * The download goes through the *shell* (`/api/kernel/download`), not through a
   * browser link, and that is deliberate: a browser download lands in the user's
   * Downloads folder and the client would still not find it. The shell fetches the
   * build for this platform from the official endpoint and writes it into `core/`
   * beside the executable, which is where `kernel::locate` looks.
   *
   * This existed before and was lost when 联机 was rewritten — the old page had both the
   * notice and the button, and the rewrite kept the notice and dropped the button, which
   * left a page that said "no kernel" with nothing to press. The flow is now stated in
   * the notice itself so the next rewrite has less to lose.
   */
  function kernelNotice(kernel) {
    var dir = (kernel && kernel.core_dir) || "core";
    var platform = kernel && kernel.platform ? kernel.platform + "-" + kernel.arch : "当前平台";
    return notice("warn",
      "<strong>还没有下载内核 <code>" + esc(kernelExpected()) + "</code>。</strong>" +
      "开房间要靠它，客户端只负责把它跑起来。<br>" +
      "当前平台 <code>" + esc(platform) + "</code> · 会装到 <code>" + esc(dir) + "</code>。") +
      '<button type="button" class="btn btn--primary room-kernel-download" data-act="kernel">' +
      icon("download", "icon--sm") + "自动下载内核</button>";
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
      // This used to point at 每日资讯 on 首页, which is no longer there — the feed
      // came off the home page with the rest of the daily content. Saying "watch the
      // news" when there is no news is worse than saying nothing, so it says nothing
      // and the card's own list is what describes the plan.
      "<p>下面的功能是计划中的样子，不代表已经可用。</p>" +
      (entry[1].length
        ? '<ul class="soon-list">' + entry[1].map(function (line) {
            return "<li>" + esc(line) + "</li>";
          }).join("") + "</ul>"
        : "") +
      '<div class="soon-actions">' +
      '<button type="button" class="btn btn--ghost" data-go="home">回到主页</button>' +
      "</div>" +
      "</div>";

    el.querySelectorAll("[data-go]").forEach(function (button) {
      button.addEventListener("click", function () { S.go("home"); });
    });
    return { title: titles[name] || "敬请期待" };
  }

  /* ------------------------------------------------------------------ home */

  function homePage(el) {
    /*
     * The games the carousel rotates through.
     *
     * Declared before the markup is built, not next to the function that reads it:
     * `gameCarousel()` is called while `el.innerHTML` is being assembled, and a
     * `var` beside its own function is still in its temporal dead zone at that
     * moment — hoisted as `undefined`, so `GAMES.map` threw and the page rendered
     * as the router's error notice.
     *
     * One game today, so one card and no dots: a single dot is furniture. The
     * rotation is written anyway, with the interval only started for more than one
     * card — the second game is meant to be a data edit, not a rewrite.
     */
    var GAMES = [
      {
        tag: "本家联机",
        title: "Minecraft",
        note: "Java 版与基岩版都在同一个隧道里。建好隧道，把地址发给朋友就行。",
        art: "asset/hero-minecraft.webp",
        // There is a 640x360 cut of this in `artwork/` beside the crate, for a narrow
        // window. It is deliberately *not* in `web/asset/`: nothing reads it yet, that
        // directory is exactly the set of files the browser can ask for, and every file
        // in it is also `include_bytes!`-ed into the binary — so a cut nobody requests is
        // download weight for every user. Switching to it means moving the file back and
        // adding either a `<picture>` or a media query here.
      },
    ];

    /*
     * Like several other layouts here, this is "a grid with a variable number of
     * items", which CSS does with `repeat(auto-fill, minmax())`. It is not
     * `repeat(auto-fit, …)`: `auto-fit` collapses the empty tracks and makes the
     * last card stretch to the full width, so a two-game row would render as one
     * wide card and one normal one. `auto-fill` leaves the empty track alone.
     */
    /*
     * The heading is not a page title any more. It was `pageHead("首页", "…")` — a page
     * named after the tab you are already on, over a sentence describing the board you
     * are already looking at — and both were furniture in the most expensive place on
     * the page. A calendar glyph and the section's own name say the same thing in one
     * line, and the item is not a destination link, so it is a div rather than an
     * anchor: there is nowhere for 页面链接 to go.
     */
    el.innerHTML =
      '<header class="board-head">' +
      '<span class="board-icon" aria-hidden="true">' +
      '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" ' +
      'stroke-linecap="round" stroke-linejoin="round">' +
      '<rect x="3" y="4.5" width="18" height="16" rx="2.5"></rect>' +
      '<path d="M3 9.5h18M8 2.5v4M16 2.5v4"></path>' +
      "</svg>" +
      "</span>" +
      '<h1 class="board-title">游戏日历</h1>' +
      "</header>" +
      '<section class="section section--first">' +
      '<div class="home-top">' + heatCalendar() + gameCarousel() + "</div>" +
      "</section>";

    var carousel = startCarousel(el);

    /* The graph is drawn asynchronously — it is a read of the shell's history, not
     * something this page can compute — and refreshed on a slow timer so a session
     * that is still running fills in today's square while the user watches. Thirty
     * seconds rather than one: the bucket only changes at 30 minutes, so the only thing
     * a faster tick buys is a request per second for a number that has not moved. */
    drawHeat();
    var heatTimer = window.setInterval(function () {
      if (S.tunnel.tunnel) drawHeat();
    }, 30000);

    /*
     * ---------------------------------------------------------------- heat map
     *
     * A calendar heat map, one cell per day, one column per week — the GitHub
     * contribution graph applied to "how much did I play". It is a record of days, so
     * it reads as a calendar: 7 rows fixed, weeks growing left to right, months
     * labelled along the top.
     *
     * The data is real: `/api/sessions` reports one row per day, aggregated by the
     * shell from `hongshi.sessions.json`, which `kernel::reap` writes when a tunnel
     * ends. That is the important part of the design — the *kernel* records the
     * session, not this page, so a browser closed mid-session cannot lose the time and
     * a page reload cannot double-count it. See `src/sessions.rs`.
     *
     * The first version of this graph invented its data, because there was no history
     * to draw. That generator is gone; the only thing still synthetic here is nothing.
     */
    function heatCalendar() {
      return '<div class="heat" id="heat">' +
        '<div class="heat-head">' +
        '<div class="heat-figures">' +
        '<p class="heat-total" id="heat-total"></p>' +
        '<p class="heat-sub" id="heat-sub"></p>' +
        "</div>" +
        '<div class="heat-legend"><span>少</span>' +
        [0, 1, 2, 3, 4].map(function (level) {
          return '<span class="heat-cell" data-level="' + level + '"></span>';
        }).join("") +
        "<span>多</span></div>" +
        "</div>" +
        '<div id="heat-body"><p class="heat-empty">正在读取记录…</p></div>' +
        "</div>";
    }

    /**
     * The last N days, as whole weeks ending on the current one.
     *
     * The range starts on a Monday and ends on a Sunday so every column is a real
     * week: a graph whose first and last columns are partial empties reads as missing
     * data rather than as the edge of the window. `rows` is keyed by `YYYY-MM-DD` and
     * may be empty — every day is then a quiet one, which is the honest picture of a
     * client nobody has played on yet.
     */
    function heatData(rows, weeks) {
      var days = [];
      var today = new Date();
      today.setHours(0, 0, 0, 0);

      var end = new Date(today);
      end.setDate(end.getDate() + (6 - ((end.getDay() + 6) % 7)));

      var start = new Date(end);
      start.setDate(start.getDate() - 7 * weeks + 1);

      for (var d = new Date(start); d <= end; d.setDate(d.getDate() + 1)) {
        var cell = new Date(d);
        var key = heatKey(cell);
        var row = rows[key] || null;
        days.push({
          date: cell,
          // The level comes from the shell (`sessions::level`), so the graph and any
          // other reader of the same file cannot disagree about what a colour means.
          level: row ? row.level : 0,
          seconds: row ? row.seconds : 0,
          sessions: row ? row.sessions : 0,
          future: cell > today,
          today: cell.getTime() === today.getTime(),
        });
      }
      return days;
    }

    /** `YYYY-MM-DD` in local time, which is how the days are bucketed for display. */
    function heatKey(date) {
      var month = String(date.getMonth() + 1);
      var day = String(date.getDate());
      if (month.length < 2) month = "0" + month;
      if (day.length < 2) day = "0" + day;
      return date.getFullYear() + "-" + month + "-" + day;
    }

    /**
     * Fold the live tunnel into the rows, so a session still running shows up now.
     *
     * Only today is touched and only by addition: the file has no record for a session
     * that has not ended, and a graph that stays grey all evening and then jumps is
     * worse than one that fills in as you play. `Math.max` rather than `+=` on the
     * level, because the level is a bucket and adding buckets is meaningless.
     */
    function withLiveTime(rows) {
      var age = S.tunnel.age();
      if (age === null || age <= 0) return rows;
      var key = heatKey(new Date());
      var row = rows[key] || { date: key, seconds: 0, sessions: 0, level: 0 };
      row.seconds += age;
      row.sessions += 1;
      row.level = Math.max(row.level, liveSessionLevel(row.seconds));
      rows[key] = row;
      return rows;
    }

    /**
     * The same four edges as `sessions::level` in Rust, for the one number Rust cannot
     * know: the session that is still running.
     *
     * Duplicating a scale in two languages is a smell, and it is the smaller one here.
     * The alternative is a round trip on every tick to ask the shell what bucket a
     * duration falls in, or not showing live time at all. The duplication is contained
     * — four numbers, and a test on the Rust side pins them — and it is commented at
     * both ends so the pair is findable.
     */
    function liveSessionLevel(seconds) {
      if (seconds <= 0) return 0;
      if (seconds < 1800) return 1;
      if (seconds < 7200) return 2;
      if (seconds < 18000) return 3;
      return 4;
    }

    function heatStats(days) {
      var total = 0;
      var seconds = 0;
      var sessions = 0;
      var streak = 0;
      var counting = true;
      for (var i = days.length - 1; i >= 0; i--) {
        var day = days[i];
        if (day.future) continue;
        if (day.seconds > 0 || day.level > 0) {
          total++;
          seconds += day.seconds;
          sessions += day.sessions;
          if (counting) streak++;
        } else if (counting) {
          // Today with nothing on it yet does not break a streak that is still
          // running — the day is not over.
          if (day.today) continue;
          counting = false;
        }
      }
      return { total: total, seconds: seconds, sessions: sessions, streak: streak };
    }

    function heatWeeks(days) {
      var months = ["1月", "2月", "3月", "4月", "5月", "6月", "7月",
        "8月", "9月", "10月", "11月", "12月"];

      var cells = days.map(function (day) {
        var label = heatLabel(day);
        return '<button type="button" class="heat-cell" data-level="' +
          (day.future ? 0 : day.level) + '"' +
          (day.today ? ' data-today="1"' : "") +
          (day.future ? ' data-future="1"' : "") +
          ' title="' + esc(label) + '" aria-label="' + esc(label) + '"></button>';
      }).join("");

      /*
       * Month labels.
       *
       * Only up to the last month that actually begins inside the window — one slot
       * per week would have been simpler, but a slot per week is 126 elements of
       * which 5 carry text, and the empty ones pushed the real labels past the right
       * edge of the card.
       *
       * Placement is as late as possible so a label never lies: 6月's first day can
       * be several columns before the window starts, and a label pinned there would
       * be outside the grid entirely. Pinning it to the first *visible* week is
       * exact; pinning it to the week after never points at the wrong month. Columns
       * later than today are skipped for the same reason — a label that appears in
       * the future is a label the graph cannot back up.
       */
      var marks = [];
      var takenWeek = {};
      days.forEach(function (day, i) {
        if (day.date.getDate() !== 1 || day.future) return;
        var week = Math.floor(i / 7);
        if (takenWeek[week]) return;
        takenWeek[week] = true;
        marks.push({ week: week, text: months[day.date.getMonth()] });
      });

      var labels = marks.map(function (mark) {
        // 13px is the grid's own column stride (10px cell + 3px gap) in app.css.
        // Writing it here is a coupling that a variable could not fix — the stride
        // lives in the stylesheet and the labels are positioned from script — so it
        // is one number in one place with a comment pointing at the other.
        return '<span class="heat-month" style="left:' + mark.week * 13 + 'px">' +
          esc(mark.text) + "</span>";
      }).join("");

      return '<div class="heat-scroll">' +
        '<div class="heat-track">' +
        '<div class="heat-months">' + labels + "</div>" +
        '<div class="heat-body">' +
        '<div class="heat-days">' +
        ["一", "", "三", "", "五", "", "日"].map(function (d) {
          return '<span class="heat-day">' + d + "</span>";
        }).join("") +
        "</div>" +
        '<div class="heat-grid">' + cells + "</div>" +
        "</div></div></div>";
    }

    function heatLabel(day) {
      var text = (day.date.getMonth() + 1) + "月" + day.date.getDate() + "日";
      if (day.future) return text + " · 还没到";
      if (day.seconds <= 0) return text + " · 没有联机";
      // The duration, not a count: the graph's colour is a duration bucket, so a
      // tooltip that said "3 次" next to a dark square would describe a different
      // quantity than the one being drawn.
      return text + " · 联机 " + S.duration(day.seconds) +
        (day.sessions > 1 ? "（" + day.sessions + " 次）" : "");
    }

    /* ------------------------------------------------------------- the loader */

    /**
     * Read the recorded days and draw the graph.
     *
     * Called once when the page renders and again on a slow timer while a room is
     * open, so the today column fills in as the session runs. The rows come from the
     * shell's own aggregation rather than from the raw file, which is what keeps the
     * level buckets in one place.
     */
    function drawHeat() {
      var box = el.querySelector("#heat-body");
      if (!box) return null;

      return S.apiJson("/api/sessions?days=126").then(function (result) {
        var body = (result && result.body) || {};
        var rows = {};
        (body.rows || []).forEach(function (row) { rows[row.date] = row; });
        withLiveTime(rows);

        var days = heatData(rows, 18);
        var stats = heatStats(days);
        box.innerHTML = heatWeeks(days);

        var total = el.querySelector("#heat-total");
        if (total) {
          total.innerHTML = stats.total > 0
            ? "<b>" + S.duration(stats.seconds) + "</b> <span>累计联机</span>"
            : "<b>还没有记录</b>";
        }
        var sub = el.querySelector("#heat-sub");
        if (sub) {
          sub.textContent = stats.total > 0
            ? "最近 4 个月 · 联机 " + stats.total + " 天 · 当前连续 " + stats.streak + " 天"
            : "开一次房间，这里就会开始记。";
        }
        return days;
      }).catch(function () {
        box.innerHTML = '<p class="heat-empty">没能读到联机记录，换个页面再回来试试。</p>';
        return null;
      });
    }

    /*
     * ------------------------------------------------------------------ games
     *
     * The cards and the dots, from `GAMES` above. Both halves are drawn from the
     * one list so the card on screen and the dot that claims to mark it cannot
     * disagree about what exists.
     */
    function gameCarousel() {
      var cards = GAMES.map(function (game, index) {
        // The art is a CSS background, so the URL is escaped into the style
        // attribute rather than into an `src`. `esc` is enough for a value this
        // file controls, and the quotes are what it has to survive. It goes through
        // `assetUrl` like every other asset: a bare `asset/…` resolves against whatever
        // route the page happens to be on, which is only the same thing while every
        // route is one segment deep.
        return '<article class="game" data-index="' + index + '"' +
          (index === 0 ? ' data-active="1"' : "") + ">" +
          '<div class="game-media" style="background-image:url(&quot;' +
          esc(S.assetUrl(game.art)) + '&quot;)" role="img" aria-label="' + esc(game.title) + '"></div>' +
          '<div class="game-body">' +
          '<span class="game-tag">' + esc(game.tag) + "</span>" +
          '<h2 class="game-title">' + esc(game.title) + "</h2>" +
          '<p class="game-note">' + esc(game.note) + "</p>" +
          "</div>" +
          "</article>";
      }).join("");

      var dots = GAMES.length > 1
        ? '<div class="game-dots">' + GAMES.map(function (game, index) {
          return '<button type="button" class="game-dot" data-index="' + index + '"' +
            (index === 0 ? ' aria-current="true"' : "") +
            ' aria-label="' + esc(game.title) + '"></button>';
        }).join("") + "</div>"
        : "";

      return '<div class="games" id="games" data-count="' + GAMES.length + '">' +
        cards + dots + "</div>";
    }

    function startCarousel(root) {
      var box = root.querySelector("#games");
      if (!box || GAMES.length < 2) return null;

      var index = 0;
      var cards = box.querySelectorAll(".game");
      var dots = box.querySelectorAll(".game-dot");

      function show(next) {
        index = (next + cards.length) % cards.length;
        for (var i = 0; i < cards.length; i++) {
          if (i === index) cards[i].dataset.active = "1";
          else delete cards[i].dataset.active;
        }
        for (var j = 0; j < dots.length; j++) {
          if (j === index) dots[j].setAttribute("aria-current", "true");
          else dots[j].removeAttribute("aria-current");
        }
      }

      for (var i = 0; i < dots.length; i++) {
        dots[i].addEventListener("click", function (event) {
          show(Number(event.currentTarget.dataset.index));
        });
      }

      var timer = window.setInterval(function () { show(index + 1); }, 6500);
      return { destroy: function () { window.clearInterval(timer); } };
    }

    /*
     * The page is the games board and nothing else, for now.
     *
     * 每日资讯, 服务 (联机隧道 / 云硬盘 / 云服务器) and 当前会话 used to fill the rest
     * of this page. They are gone on purpose while 首页 is reworked: the daily feed
     * is a frozen 2024 Mojang launcher batch that belongs with 资源查找 rather than
     * above it, and the service cards duplicated what 联机 and 设置 already say. The
     * blank half of the page is reserved for the daily content that replaces them.
     *
     * Their builders went with them — `renderNews`, `unavailableCard`, `tunnelCard`,
     * `renderServices`, `renderSession` and the `S.apiJson("/api/daily-news")` read —
     * rather than being left behind as dead code that would have to be read and ruled
     * out every time this file is opened. `/api/daily-news` is still served:
     * `site.rs` and its tests are untouched, so a page can ask for it again without
     * anything on the Rust side changing.
     *
     * There is no `onTick` and no `onHealth` here any more either. Both existed only
     * to rewrite the session card and the tunnel counters, and a page that returns
     * neither is not given a tick at all — see `show()` in app.js.
     *
     * The two timers are cleared here and nowhere else, which is the whole reason this
     * is one `return` instead of the `return carousel || NO_CAROUSEL` it was a moment
     * ago: a `return` above this comment makes everything below it dead code, and the
     * thing that was dead was the cleanup. The heat timer leaked a 30-second interval
     * on every visit to 首页 — invisible, because a leaked timer that redraws an
     * element that no longer exists does nothing except keep running.
     */
    return {
      title: "首页",
      destroy: function () {
        if (carousel && carousel.destroy) carousel.destroy();
        window.clearInterval(heatTimer);
      },
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

  /*
   * --------------------------------------------------------------- connect
   *
   * 联机 is one decision and one button.
   *
   * It used to be a form: 转发方式 (中转 / P2P), 协议 (TCP / UDP), 本机地址, a relay
   * listbox and a tunnel card — and only the first option of each was ever selectable.
   * Four controls that say "no" are not a choice, they are a reading assignment: the
   * user has to work out which of them matter before they can reach the one button
   * that does anything. It is now: pick a relay, accept the port, press 开启房间.
   *
   * The two controls that remain are the two the user can actually answer. The relay
   * is a real choice — which node to share through. The port is not really a choice,
   * it is a fact about where the game is listening, so it is *detected* and shown and
   * the field exists to correct the detection rather than to demand an answer. An
   * empty field is filled from `/api/ports/detect`; a field the user has touched is
   * left alone for the life of the page.
   *
   * 转发方式 and 协议 did not stop existing — the kernel still only does relayed TCP.
   * They went because stating them at the user as if they were decisions is worse than
   * saying nothing: the honest version is that they are the same for everyone. When
   * P2P or UDP ships, this page grows a control that does something.
   *
   * The relay list itself is `pickerRows()`, the same rows the old page built, so the
   * button and the open list still cannot disagree about what exists.
   */

  /**
   * How long 开启房间 keeps claiming the address might still arrive, in ms.
   *
   * It was 4500, chosen as "about as long as a relay handshake takes", and that was a
   * guess that failed on the first real run: the kernel took roughly five seconds, so
   * the dialog declared failure while the page behind it was already showing the
   * address. The dialog now opens immediately and fills itself in, so this is only the
   * point at which the *pending* wording stops being honest — and a user tired of
   * waiting can close it at any time, which is what makes a generous number affordable.
   */
  var ROOM_ENDPOINT_WAIT_MS = 15000;
  var ROOM_ENDPOINT_POLL_MS = 500;

  /**
   * A relay's three readings, as a word and a tone.
   *
   * The state comes from the shell's own probe (`/api/probe` → `probe_state`), which
   * already separates "the control port answered" from "only ICMP did" — and that
   * separation is the whole point, because a node you can ping but not tunnel through
   * is not a usable node and calling it 「稳定」would be a lie the user pays for with a
   * dead address they handed to a friend.
   *
   *   ok                       TCP 控制端口答了          → 延迟稳定   green
   *   slow                     answered, but slowly      → 较稳定     amber
   *   ping                     ping only, port closed    → 仅能 ping 通 amber
   *   dead / none / no answer  nothing answered          → 断联       bright red
   *
   * The last four are the latency ladder, which is what `nodeState()` falls back to
   * for a node the probe has not reported on yet — `low/mid/high` are milliseconds,
   * not verdicts, so they map onto the same three readings rather than being shown
   * raw. Every name here was read off `NODE_STATE` / `LATENCY_LABEL` in app.js
   * rather than guessed: the first version of this function tested for `"icmp"`,
   * which is a state this shell has never produced.
   */
  function relayReading(row) {
    var state = row ? row.state : "unknown";
    if (state === "ok") return { dot: "ok", label: "延迟稳定" };
    if (state === "slow" || state === "mid") return { dot: "warn", label: "较稳定" };
    if (state === "ping") return { dot: "warn", label: "仅能 ping 通" };
    if (state === "low") return { dot: "ok", label: "延迟稳定" };
    if (state === "high") return { dot: "warn", label: "较慢" };
    if (state === "dead" || state === "none") return { dot: "down", label: "断联" };
    return { dot: "busy", label: "测速中…" };
  }

  /* The page's mark is the program's own icon — the same `icon.png` the top bar uses,
     so 联机 is branded like the client rather than like a concept.

     It was an inline house SVG, drawn because a house says 房间 and because there is no
     house glyph in the sprite. But the heading already says 红石远程联机, so a generic
     house beside it was a second, weaker answer to a question the words had answered;
     the product's own mark is the one image that means *this* app. Two icons with the
     same brand in them is the point, not a duplication: the bar's is 30px and this one
     is 56px, and they are read at different moments.

     `assetUrl` rather than a bare path: the helper is the one place that knows how a
     shell asset is addressed, and the same rule covers the runtime case where the page
     is served from disk instead of from the binary. */
  var ROOM_ICON = '<span class="room-hero-mark" role="img" aria-label="红石联机" ' +
    'style="background-image:url(&quot;' + esc(S.assetUrl("icon.png")) + '&quot;)"></span>';

  var COPY_ICON =
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" ' +
    'stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' +
    '<rect x="9" y="9" width="11" height="11" rx="2"></rect>' +
    '<path d="M6 15H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h8a2 2 0 0 1 2 2v1"></path></svg>';

  function connectPage(el) {
    var selected = "auto";
    var manualPort = false;
    var detected = null;
    var timer = null;
    var waiting = null;
    var unsubscribe = null;
    var unsubscribeNodes = null;
    var unsubscribeKernel = null;
    /* Whether *this* room ever reached the point of having an address. Read once, when its
       dialog is dismissed; see `openRoomDialog`. */
    var roomOpened = false;

    el.innerHTML =
      '<div class="room">' +
      '<header class="room-hero">' +
      '<span class="room-hero-icon">' + ROOM_ICON + "</span>" +
      '<h1 class="room-hero-title">红石远程联机</h1>' +
      "</header>" +
      '<div id="room-body"></div>' +
      "</div>";

    var body = el.querySelector("#room-body");

    /*
     * Two delegated listeners on the page element, not one per control.
     *
     * Both `render()` and every store publication replace the contents of `#room-body`,
     * so listeners bound to the buttons inside it would have to be rebound on each
     * pass. Delegation is bound once to the element the router owns.
     *
     * It also has to be *removed* on destroy, and that is a rule this project learned
     * the hard way: the old 联机 page delegated on `el` and never took the listeners
     * off, so returning to it stacked handlers and one click started the tunnel twice —
     * a success and an error from one press, because the second call was refused with
     * 已经有一个隧道在运行了. `verify.ps1` asserts the counts match.
     */
    function onPageClick(event) {
      var target = event.target.closest ? event.target.closest("[data-act]") : null;
      if (!target || !body.contains(target)) return;
      var act = target.dataset.act;
      if (act === "start") startRoom(target);
      else if (act === "stop") stopRoom(target);
      else if (act === "kernel") downloadKernel(target);
      else if (act === "detect") detectPort(true);
      else if (act === "copy") S.copyText((S.tunnel.tunnel || {}).endpoint || "", "连接地址");
      else if (act === "relay") {
        var list = body.querySelector("#room-relay-list");
        if (list && list.hidden) openPicker();
        else closePicker();
      } else if (act === "choose") {
        selected = target.dataset.value;
        closePicker();
        render();
      }
    }

    function onPageInput(event) {
      if (event.target && event.target.id === "room-port") {
        manualPort = true;
        portHint();
      }
    }

    function onPageKeydown(event) {
      if (event.target && event.target.id === "room-relay" &&
          (event.key === "ArrowDown" || event.key === "Enter" || event.key === " ")) {
        event.preventDefault();
        openPicker();
      }
    }

    el.addEventListener("click", onPageClick);
    el.addEventListener("input", onPageInput);
    el.addEventListener("keydown", onPageKeydown);

    /* ------------------------------------------------------------------ render */

    function rowFor(value) {
      var rows = pickerRows();
      for (var i = 0; i < rows.length; i++) {
        if (rows[i].value === value) return rows[i];
      }
      return rows[0];
    }

    function render() {
      var tunnel = S.tunnel.tunnel;
      body.innerHTML = tunnel ? roomRunning(tunnel) : roomIdle();
      wire(tunnel);
    }

    function roomIdle() {
      var row = rowFor(selected);
      var reading = relayReading(row);
      var kernel = S.kernel.info;
      var found = !!(kernel && kernel.found);

      return '<div class="room-controls">' +
        '<div class="room-field"><span class="room-label">中转服务器</span>' +
        '<div class="room-select">' +
        '<button type="button" class="input room-trigger" id="room-relay" data-act="relay" ' +
        'aria-haspopup="listbox" aria-expanded="false" aria-label="选择中转服务器">' +
        '<span class="picker-text"><span class="picker-name">' + esc(row.name) + "</span></span>" +
        '<span class="room-reading" data-tone="' + reading.dot + '" id="room-relay-state">' +
        '<span class="dot" data-latency="' + reading.dot + '"></span>' +
        esc(reading.label) + "</span>" +
        '<span class="room-caret" aria-hidden="true"></span>' +
        "</button>" +
        '<div class="picker-list room-list" id="room-relay-list" role="listbox" ' +
        'aria-label="中转服务器" hidden></div>' +
        "</div>" +
        '<p class="room-hint" id="room-relay-hint"></p>' +
        "</div>" +
        '<div class="room-field"><label class="room-label" for="room-port">本地游戏端口</label>' +
        '<div class="room-port-row">' +
        '<input class="input input--mono" id="room-port" type="number" inputmode="numeric" ' +
        'min="1" max="65535" autocomplete="off" placeholder="自动探测">' +
        '<button type="button" class="btn btn--ghost room-redetect" id="room-redetect" ' +
        'data-act="detect" title="重新探测本机游戏端口" aria-label="重新探测本机游戏端口">' +
        icon("refresh", "icon--sm") + "</button>" +
        "</div>" +
        '<p class="room-hint" id="room-port-hint"></p>' +
        // The port is the one field on this page that asks for a fact the user may not
        // have. The guide explains it once; this link is how they find the explanation
        // again on the second run, when the guide no longer appears.
        '<a class="room-link" href="/help/port">什么是游戏端口？</a>' +
        "</div>" +
        "</div>" +
        (found ? "" : '<div id="room-blocked"></div>') +
        '<button type="button" class="btn btn--primary room-action" id="room-start" data-act="start"' +
        (found ? "" : " disabled") + ">" + icon("play", "icon--sm") + "开启房间</button>";
    }

    function roomRunning(tunnel) {
      return '<div class="room-running">' +
        '<p class="room-label">把下面这行发给朋友，他们在游戏里填这个地址就能进来</p>' +
        '<div class="room-address">' +
        '<div class="room-address-value' + (tunnel.endpoint ? "" : " is-empty") + '">' +
        esc(tunnel.endpoint || "正在分配地址…") + "</div>" +
        '<button type="button" class="btn btn--ghost room-copy" id="room-copy" data-act="copy"' +
        (tunnel.endpoint ? "" : " disabled") + ">" + COPY_ICON + "复制地址</button>" +
        "</div>" +
        '<p class="room-hint" id="room-age"></p>' +
        /*
         * The two answers a host needs the moment the address exists: what to tell the
         * friend, and what to do when the friend says it does not work. They live on 帮助
         * rather than in the page because they are read once, by somebody who is stuck.
         */
        '<p class="room-links">' +
        '<a class="room-link" href="/help/join">朋友怎么加入房间？</a>' +
        '<a class="room-link" href="/help/trouble">常见问题：朋友连不上怎么办？</a>' +
        "</p>" +
        "</div>" +
        '<button type="button" class="btn btn--danger room-action" id="room-stop" data-act="stop">' +
        icon("close", "icon--sm") + "关闭房间</button>";
    }

    /*
     * Everything that is not a click.
     *
     * The clicks are delegated (see `onPageClick`); what is left is the one thing that
     * has to happen when the markup is *replaced* rather than when it is clicked — the
     * detected port has to be put back into a field the re-render just recreated, and
     * the kernel notice has to be drawn from whatever the kernel store knows now.
     */
    function wire(tunnel) {
      if (tunnel) {
        renderAge();
        return;
      }

      var port = body.querySelector("#room-port");
      if (port && detected && !manualPort) port.value = String(detected.port);

      var blocked = body.querySelector("#room-blocked");
      if (blocked) blocked.innerHTML = kernelNotice(S.kernel.info);

      relayHint();
      portHint();
    }

    /* ------------------------------------------------------------------ relay */

    function openPicker() {
      var list = body.querySelector("#room-relay-list");
      var trigger = body.querySelector("#room-relay");
      if (!list || !trigger) return;

      list.innerHTML = pickerRows().map(function (row) {
        var reading = relayReading(row);
        return '<button type="button" class="picker-option" role="option" data-act="choose" ' +
          'data-value="' + esc(row.value) + '" aria-selected="' +
          (row.value === selected ? "true" : "false") + '">' +
          '<span class="picker-text"><span class="picker-name">' + esc(row.name) + "</span>" +
          (row.host
            ? '<span class="picker-host">' + esc(row.host) + "</span>"
            : '<span class="picker-note">' + esc(row.note) + "</span>") +
          "</span>" +
          '<span class="picker-meta"><b>' +
          esc(typeof row.ms === "number" ? row.ms + " ms" : "—") + "</b>" +
          '<span class="room-reading" data-tone="' + reading.dot + '">' +
          '<span class="dot" data-latency="' + reading.dot + '"></span>' +
          esc(reading.label) + "</span></span>" +
          "</button>";
      }).join("");

      list.hidden = false;
      trigger.setAttribute("aria-expanded", "true");
    }

    function closePicker() {
      var list = body.querySelector("#room-relay-list");
      var trigger = body.querySelector("#room-relay");
      if (list) list.hidden = true;
      if (trigger) trigger.setAttribute("aria-expanded", "false");
    }

    function relayHint() {
      var hint = body.querySelector("#room-relay-hint");
      if (!hint) return;
      var row = rowFor(selected);
      if (!row) {
        hint.textContent = "正在读取节点列表…";
        return;
      }
      if (row.value === "auto") {
        hint.textContent = "自动选择：开房间时用测速最快、并且能建隧道的那个节点。";
        return;
      }
      var reading = relayReading(row);
      hint.textContent = reading.dot === "down"
        ? "这个节点现在建不了隧道，换一个再开。"
        : "房间会通过这个节点中转。";
    }

    /* ------------------------------------------------------------------- port */

    /**
     * What to say under the port field.
     *
     * The three sources are three different claims and the words have to match, because
     * the user is about to hand this number to somebody else:
     *
     *   default   a listening port in 25565–25569 held by a Java process — this is the game
     *   java      a listening port held by a Java process, somewhere else — this is the game
     *   fallback  nothing was found — this is the default, and it is a guess
     *
     * A single "已探测到" for all three would be a lie in the third case, which is the case
     * where the user most needs to know.
     */
    function portHint() {
      var hint = body.querySelector("#room-port-hint");
      if (!hint) return;
      if (manualPort) {
        hint.textContent = "手动填写的端口。";
        return;
      }
      if (!detected) {
        hint.textContent = "正在探测本机游戏端口…";
        return;
      }
      if (detected.source === "fallback") {
        hint.textContent = "没找到正在运行的游戏，先按默认端口 " + detected.port + " 走；" +
          "开着游戏再点右边的刷新按钮。";
        return;
      }
      var where = detected.source === "default" ? "默认端口" : "非默认端口";
      hint.textContent = "已探测到本机游戏在 " + detected.port + " 端口（" + where + "）。";
    }

    /**
     * Ask the shell where the game is, and put the answer in the field.
     *
     * `forced` is the 重新探测 button. The difference matters and is the whole reason the
     * parameter exists: the first call must not overwrite a port the user typed, while the
     * button is the user *asking* for it to be overwritten — pressing it after starting a
     * game is the exact flow it is there for.
     */
    function detectPort(forced) {
      if (manualPort && !forced) return;
      if (forced) {
        manualPort = false;
        var hint = body.querySelector("#room-port-hint");
        if (hint) hint.textContent = "正在探测本机游戏端口…";
      }

      S.apiJson("/api/ports/detect").then(function (result) {
        var found = (result && result.body) || null;
        if (!found || typeof found.port !== "number") {
          portHint();
          return;
        }
        detected = found;
        if (manualPort) return;
        var port = body.querySelector("#room-port");
        if (port) port.value = String(found.port);
        portHint();
      }).catch(function () {
        portHint();
      });
    }

    /* ------------------------------------------------------------------ start */

    function startRoom(button) {
      var portField = body.querySelector("#room-port");
      var port = portField && portField.value ? parseInt(portField.value, 10) : 0;
      if (!(port > 0 && port < 65536)) {
        S.toast("本地游戏端口要填 1 到 65535 之间的数字", "warn");
        if (portField) portField.focus();
        return;
      }

      /*
       * The relay has to be resolved here, and getting this wrong is not subtle.
       *
       * `selected` is a *choice* — the string `"auto"` is this page's own word for
       * "whichever is fastest" — and the kernel takes a hostname. The first version of
       * this function passed `selected` straight through, so 自动选择 sent `-t auto` and
       * the kernel answered `不知道这样的主机`, exited 1, and the room never came up. The
       * dialog said 房间没开起来 and was right, but the default path was broken, which is
       * the worst place for a bug to live.
       *
       * `store.best()` is the same resolution the old page used: lowest measured
       * latency among the nodes a tunnel can actually be built on, not merely the
       * lowest.
       */
      var node = selected === "auto" ? store.best() : null;
      if (!node) {
        node = store.nodes.filter(function (entry) { return entry.host === selected; })[0] || null;
      }
      if (!node) {
        S.toast(selected === "auto"
          ? "还没有可用的中转节点，先刷新一下节点列表"
          : "选中的节点不在列表里了，重新选一个", "warn");
        return;
      }
      if (selected !== "auto" && relayReading({ state: nodeState(node) }).dot === "down") {
        S.toast("这个节点现在连不上，换一个节点再开房间", "warn");
        return;
      }

      var button = body.querySelector("#room-start");
      if (button) {
        button.disabled = true;
        button.textContent = "正在开启…";
      }

      /*
       * Do **not** put `data.tunnel` into the store here.
       *
       * A start request answers as soon as the process is spawned, and the tunnel it
       * carries has `endpoint: null` and `uuid: null` — the address arrives later on
       * the kernel's stdout. `S.tunnel.followKernel` is what promotes those fields, and
       * it skips the write when endpoint/uuid/node are unchanged, so seeding the store
       * with the empty version made every later poll a no-op: the store sat on
       * `endpoint: null` forever while `/api/tunnel/status` reported the real address,
       * the page said 正在分配地址…, and `awaitRoom` timed out and announced a failure
       * for a tunnel that was up.
       *
       * Nothing has to be set: `S.tunnel` is refreshed on the boot interval and
       * `followKernel` publishes as soon as the kernel reports an endpoint, which is
       * what re-renders this page into its running state.
       */
      /*
       * Start the kernel poll, and this is the line that makes the running state work
       * at all.
       *
       * `kernelStore.watch` is armed in exactly two places: `drawerOpen(true)`, and
       * `boot()` when the kernel was *already* running at startup. The old page opened
       * the log drawer on start, so it got the poll as a side effect. This page
       * deliberately does not open the drawer — the address is handed over in a dialog
       * instead — and removing that call silently removed the only thing that ever told
       * the client a kernel had come up. The symptom was a tunnel that the server
       * reported correctly (`/api/tunnel/status` had the endpoint) while the page said
       * 正在分配地址… forever with `S.kernel.info.state === "stopped"`.
       *
       * `watch(true)` is a fact about the world, not about the panel: a kernel is
       * running, so the client polls for it. A poll that finds nothing only re-arms at
       * the idle rate — see `rearm` — so this stays cheap.
       */
      S.kernel.watch(true);

      // This room has not produced an address yet. Reset per attempt: a second room in the
      // same visit must earn its own dismissal.
      roomOpened = false;

      S.apiSend("/api/tunnel/start", { relay: node.host, game_port: port }).then(function (result) {
        var data = (result && result.body) || {};
        if (!result.ok || data.state !== "ok") {
          // The shell refuses in words on purpose — 已经有一个隧道在运行 / 没有找到内核 /
          // 没有选择中转服务器 — and those words are worth more than a generic failure.
          S.toast(data.reason || "开启失败，换个节点再试一次", "warn");
          if (button) {
            button.disabled = false;
            button.innerHTML = icon("play", "icon--sm") + "开启房间";
          }
          return;
        }
        awaitRoom(data.tunnel);
      }).catch(function (err) {
        S.toast("客户端没有响应：" + String(err), "warn");
        if (button) {
          button.disabled = false;
          button.innerHTML = icon("play", "icon--sm") + "开启房间";
        }
      });
    }

    /*
     * Wait for the endpoint, then say it out loud.
     *
     * A start request answers as soon as the process is spawned; the address arrives
     * later on the kernel's stdout and the shell reads it from there. So there is a
     * second or two where a room exists and has no address, and the page has to hold
     * the user through it rather than opening a dialog with a blank line in it.
     *
     * The dialog is the point of the whole flow: the address is what the user came
     * for, and making them hunt for it in a log panel was the old behaviour. If the
     * endpoint never arrives the dialog opens anyway and says so, because "it did not
     * come up" is the one thing a user must not have to infer from silence.
     */
    function awaitRoom(seed) {
      var deadline = Date.now() + ROOM_ENDPOINT_WAIT_MS;
      window.clearTimeout(waiting);

      /*
       * The dialog opens *first*, as pending, and the address is filled into it when it
       * arrives.
       *
       * The first version waited for the endpoint and only then opened, with a 4.5s cap
       * after which it announced failure. That is a false negative waiting to happen:
       * it did happen, on a run where the kernel took about five seconds to reach the
       * relay — the dialog said 房间没开起来 while the page behind it was already
       * showing `nj.hongshi.site:49048`. A fixed wait cannot tell "not yet" from "not
       * ever", so it should not try: open immediately, say 正在连接, and update in
       * place. The deadline now only decides when to stop claiming it might still work.
       */
      var panel = openRoomDialog();
      var startedAt = Date.now();
      /*
       * Latched, and latching is the whole fix.
       *
       * Three versions of this condition reported failure over a room that was coming
       * up, and all three shared one mistake: they treated *not having observed* a
       * running kernel as evidence that there was none.
       *
       *   1. `!current` — the tunnel store is empty for the first second after any start.
       *   2. `S.kernel.info && !S.kernel.info.running` — that snapshot is the *previous*
       *      kernel for a moment, so a room opened just after closing one died at once.
       *   3. the same check behind a 2.5s grace window — still wrong, because the store
       *      can go longer than that without reporting anything at all: it publishes only
       *      when the state *changes*, and the last thing it heard was "stopped".
       *
       * So the fact being waited for is positive: "this client has seen the kernel of
       * this room running". Only once that is true does its absence mean anything, and
       * then it means the kernel died — which is worth reporting immediately instead of
       * holding the user until the deadline.
       *
       * The last resort is the deadline, and it says nothing about the kernel because by
       * then the client genuinely does not know: the room may still be connecting.
       */
      var sawRunning = false;

      function look() {
        var current = S.tunnel.tunnel;
        if (current && current.endpoint) {
          fillRoomDialog(panel, current.endpoint, seed);
          return;
        }

        var kernel = S.kernel.info;
        if (kernel && kernel.running) sawRunning = true;
        if (sawRunning && (!kernel || !kernel.running)) {
          markRoomDialogFailed(panel, "内核起来之后又退出了。");
          return;
        }
        if (Date.now() >= deadline) {
          markRoomDialogFailed(panel, "过了这么久还没拿到地址。");
          return;
        }
        waiting = window.setTimeout(look, ROOM_ENDPOINT_POLL_MS);
      }
      look();
    }

    /*
     * The room dialog, on the shell's shared dialog (`S.openDialog`).
     *
     * It keeps only what is specific to a room: the address box, its three states and the
     * copy button. The overlay, the panel, the backdrop click and Escape used to be
     * written here too, and they are now the same code the version notice and the
     * first-run guide use — three copies of an overlay is how a client ends up with
     * three of everything.
     */
    function openRoomDialog() {
      return S.openDialog(
        '<h2 class="dlg-title" id="room-dialog-title">房间正在连接</h2>' +
        "<p>客户端已经在启动中转连接，地址一出来就显示在下面。</p>" +
        '<div class="dlg-addr is-pending" id="room-dialog-addr">正在分配地址…</div>' +
        '<p class="dlg-note" id="room-dialog-note">' +
        "这一般要几秒。地址出现之前，先别把空地址发给朋友。</p>" +
        S.dialogActions(
          '<button type="button" class="btn btn--ghost" data-dialog-close>知道了</button>'
        ),
        null,
        false,
        /*
         * Dismissed — by 知道了, by Escape, or by a click on the backdrop.
         *
         * `roomOpened` is the gate on what happens next: the tenth-launch thank-you is
         * only owed to somebody who actually played, and a room that never came up is a
         * failed attempt rather than a session. So the address arriving is what arms it,
         * and this callback is what fires it.
         */
        function () {
          if (roomOpened) S.announceSupport();
        }
      );
    }

    /** The address arrived: fill it in, and give the user the button that uses it. */
    function fillRoomDialog(panel, endpoint, seed) {
      if (!panel || !panel.isConnected) return;
      // The room is real: this is what makes the dismissal worth acknowledging. See the
      // `onClose` callback in `openRoomDialog`.
      roomOpened = true;
      var tunnel = S.tunnel.tunnel || {};
      var host = (seed && seed.node) || tunnel.node || "自动选择";

      panel.querySelector("#room-dialog-title").textContent = "房间开好了";
      var body = panel.querySelector("p");
      if (body) {
        body.textContent = "把下面这行发给朋友，他们在游戏里「多人游戏 → 直接连接」填进去就能进。";
      }
      var address = panel.querySelector("#room-dialog-addr");
      address.classList.remove("is-pending");
      address.textContent = endpoint;

      var note = panel.querySelector("#room-dialog-note");
      if (note) {
        note.textContent = "通过 " + host +
          (tunnel.game_port ? " · 本地端口 " + tunnel.game_port : "") +
          " · 房间开着就一直有效";
      }

      var actions = panel.querySelector(".dlg-actions");
      var copy = document.createElement("button");
      copy.type = "button";
      copy.className = "btn btn--primary";
      copy.id = "room-dialog-copy";
      copy.innerHTML = COPY_ICON + "复制地址";
      copy.addEventListener("click", function () {
        S.copyText(endpoint, "连接地址");
        copy.textContent = "已复制";
        copy.disabled = true;
      });
      actions.insertBefore(copy, actions.firstChild);
    }

    /** The wait ran out, or the room died. Say so in the dialog the user is holding. */
    function markRoomDialogFailed(panel, reason) {
      if (!panel || !panel.isConnected) return;
      panel.querySelector("#room-dialog-title").textContent = "房间没开起来";
      var body = panel.querySelector("p");
      if (body) {
        body.textContent = reason + "可能是没能连上中转服务器，或者网络断了。";
      }
      var address = panel.querySelector("#room-dialog-addr");
      address.textContent = "没有地址";
      address.classList.remove("is-pending");
      address.classList.add("is-failed");
      var note = panel.querySelector("#room-dialog-note");
      if (note) note.textContent = "换个节点再开一次试试。";
    }

    /* ------------------------------------------------------------------- stop */

    /**
     * Fetch the kernel from the official site and install it beside the client.
     *
     * No log panel and no terminal output: an install is one action with one outcome,
     * and the button plus a toast is the whole story. What matters is the *end state* —
     * `/api/kernel/download` reports the path it wrote, and `S.kernel.refresh()` is what
     * makes the notice disappear and 开启房间 come alive, because the kernel store is the
     * one place that decides whether a kernel exists.
     */
    function downloadKernel(button) {
      var label = button.innerHTML;
      button.disabled = true;
      button.textContent = "正在下载…";

      S.kernel.download().then(function (result) {
        var body = (result && result.body) || {};
        if (!result.ok) {
          S.toast("下载内核失败：" + (body.reason || "原因未知"), "warn", 8000);
          return;
        }
        S.toast("内核已装好（" + Math.round((body.bytes || 0) / 1024) + " KB），可以开房间了", null, 6000);
        return S.kernel.refresh();
      }).catch(function (err) {
        S.toast("下载内核失败：" + String(err), "warn", 8000);
      }).then(function () {
        // The button may be gone by now — a successful download re-renders this page —
        // so it is checked rather than assumed.
        if (button.isConnected) {
          button.disabled = false;
          button.innerHTML = label;
        }
      });
    }

    function stopRoom(button) {
      if (button) {
        button.disabled = true;
        button.textContent = "正在关闭…";
      }
      // No log panel here either. Closing a room is not a thing to go and read about;
      // it either happened or the toast says why it did not.
      S.tunnel.stop().then(function () {
        S.toast("房间已关闭，记录已存好", null);
      }).catch(function (err) {
        S.toast("关闭失败：" + String(err), "warn");
      });
    }

    /* ----------------------------------------------------------------- wiring */

    function renderAge() {
      var hint = body.querySelector("#room-age");
      if (!hint) return;
      var age = S.tunnel.age();
      var tunnel = S.tunnel.tunnel || {};
      hint.textContent = "房间已开启 " + (age === null ? "—" : S.duration(age)) +
        (tunnel.node ? " · 通过 " + tunnel.node : "");
    }

    /*
     * Two subscriptions, because two things this page shows arrive on their own time.
     *
     * The relay's stability reading comes from the node store, which is read once at
     * startup and re-probed when the user asks — so the page has to *ask* for it
     * (`load(false)` answers from cache when `boot()` already read it) and then redraw
     * when the measurement lands. Without the subscribe the indicator sat on
     * 「测速中…」for the life of the page: the store had the numbers, this page just
     * never heard about them.
     *
     * The other is the tunnel, which changes when the kernel says so.
     */
    unsubscribe = S.tunnel.subscribe(function () {
      // Re-rendering across the idle/running boundary throws the open list away, so a
      // poll landing while it is open closes it. Same trade the old picker made, and
      // the right one: a list that reshuffles under the cursor is worse than one that
      // closes.
      render();
    });
    unsubscribeNodes = store.subscribe(function () {
      // Only the reading and the hint change, so the trigger and the port field are
      // updated in place: a full re-render here would wipe a half-typed port, and the
      // port has nothing to do with which relay is fastest.
      var row = rowFor(selected);
      var reading = relayReading(row);
      var label = body.querySelector("#room-relay-state");
      if (label) {
        label.dataset.tone = reading.dot;
        var dot = label.querySelector(".dot");
        if (dot) dot.dataset.latency = reading.dot;
        // The last text node is the phrase; the dot is the first child.
        label.lastChild.textContent = reading.label;
      }
      var name = body.querySelector("#room-relay .picker-name");
      if (name) name.textContent = row.name;
      relayHint();
    });

    /*
     * And a third, for the kernel.
     *
     * `S.kernel.info` is null until `kernelStore.refresh()` answers, which lands after
     * this page's first render — so the first paint said 没有找到内核 and disabled 开启房间
     * on a machine that has one. Without this subscription that state was permanent: the
     * button stayed disabled and the notice stayed on screen for the life of the page.
     * Measured, not guessed — the first build of this page did exactly that.
     *
     * Targeted rather than a re-render, for the same reason as the node subscription:
     * the button and the notice are the only two things here that depend on the kernel,
     * and a re-render would wipe a half-typed port.
     */
    unsubscribeKernel = S.kernel.subscribe(function () {
      var kernel = S.kernel.info;
      var found = !!(kernel && kernel.found);
      var button = body.querySelector("#room-start");
      if (button) button.disabled = !found;
      var blocked = body.querySelector("#room-blocked");
      if (blocked) blocked.innerHTML = found ? "" : kernelNotice(kernel);
    });

    render();
    detectPort();
    store.load(false);

    /* One second, and only the age line is rewritten.
     *
     * The duration is the only thing on this page that changes on a clock, and the
     * page the user is looking at while a room is open is exactly the one where a
     * frozen counter reads as "did it die?". Re-rendering the idle half on a timer
     * would replace the port field mid-typing, so it does not. */
    timer = window.setInterval(function () {
      if (S.tunnel.tunnel) renderAge();
    }, 1000);

    return {
      title: "联机",
      onHealth: function () { relayHint(); },
      destroy: function () {
        // The delegated listeners first: they are on the element the router reuses for
        // every page, so a leftover would fire on the *next* page's markup.
        el.removeEventListener("click", onPageClick);
        el.removeEventListener("input", onPageInput);
        el.removeEventListener("keydown", onPageKeydown);

        window.clearInterval(timer);
        window.clearTimeout(waiting);
        closePicker();
        // The dialog belongs to the shell now, so it is closed the same way the version
        // notice and the guide are — and it has to be closed here, or leaving the page
        // leaves a room dialog floating over the next one.
        S.closeDialog();

        if (unsubscribe) unsubscribe();
        if (unsubscribeNodes) unsubscribeNodes();
        if (unsubscribeKernel) unsubscribeKernel();
      },
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
      /*
       * 个性化.
       *
       * The picture is a **path**, not an upload, and the difference is the whole
       * design of this section. A browser cannot tell a page where a dropped file
       * lives — `File` carries a name, a size and a timestamp and no path — so an
       * "upload" here could only mean copying the user's picture into the client's
       * own folder, which is a second copy of a 20 MB wallpaper to keep in step and
       * to clean up. The shell reads the file where it is instead, and the price is
       * that the path has to be typed: the drag and the file dialog are *hints* that
       * fill in the file name, and the sentence under the field says so.
       *
       * 恢复内置背景 is a real button rather than "clear the box and save", because
       * the thing a user wants back is a picture, not an empty string.
       */
      section("个性化", "image",
        '<div class="settings-form">' +
        '<div class="field">' +
        '<div class="bg-label-row"><label class="label" for="set-bgpath">背景图片</label>' +
        pill("set-bgstate", "down", "内置背景") + "</div>" +
        '<div class="bg-drop" id="set-bgdrop">' +
        '<input class="input input--mono" id="set-bgpath" type="text" autocomplete="off" ' +
        'spellcheck="false" placeholder="图片的完整路径，例如 C:\\Users\\你\\Pictures\\bg.jpg">' +
        '<button type="button" class="btn btn--ghost" id="set-bgbrowse">' +
        icon("image") + "浏览…</button>" +
        "</div>" +
        '<input type="file" id="set-bgfile" accept="' + BACKGROUND_ACCEPT + '" hidden>' +
        '<p class="section-note bg-hint" id="set-bgwhy"></p>' +
        "</div>" +
        '<div class="field-row">' +
        '<div class="field"><div class="range-head"><label class="label" for="set-bgblur">模糊度</label>' +
        '<span class="range-value" id="set-bgblur-value">0 px</span></div>' +
        '<input type="range" id="set-bgblur" min="0" max="40" step="1" value="0"></div>' +
        '<div class="field"><div class="range-head"><label class="label" for="set-bgdark">背景变暗</label>' +
        '<span class="range-value" id="set-bgdark-value">62%</span></div>' +
        '<input type="range" id="set-bgdark" min="0" max="100" step="1" value="62"></div>' +
        "</div>" +
        '<div class="field" id="set-bgcropfield" hidden>' +
        '<div class="range-head"><label class="label" for="set-bgstage">裁剪区域</label>' +
        '<span class="range-value" id="set-bgcrop-readout"></span></div>' +
        '<div class="crop-stage" id="set-bgstage" role="application" ' +
        'aria-label="拖动方框选择要显示的画面，四角可以缩放">' +
        '<img class="crop-image" id="set-bgimage" alt="" draggable="false">' +
        '<div class="crop-box" id="set-bgbox">' +
        '<span class="crop-handle" data-corner="nw"></span>' +
        '<span class="crop-handle" data-corner="ne"></span>' +
        '<span class="crop-handle" data-corner="sw"></span>' +
        '<span class="crop-handle" data-corner="se"></span>' +
        "</div></div>" +
        '<div class="crop-actions">' +
        '<button type="button" class="btn btn--ghost btn--small" id="set-bgcrop-reset">' +
        icon("refresh", "icon--sm") + "重置裁剪</button>" +
        "</div></div>" +
        '<div><button type="button" class="btn btn--ghost" id="set-bgclear">' +
        icon("cross", "icon--sm") + "恢复内置背景</button></div>" +
        "</div>",
        "图片不会被复制：客户端只记住它在哪，用的时候直接读那个文件。" +
        "所以这里要的是<b>完整路径</b>——浏览器出于安全不会把拖进来的文件的路径交给我们，" +
        "拖拽和「浏览」只会帮你填上文件名，目录要自己补。" +
        "原图被移动或删除后，会自动退回内置背景。") +
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

    // The running build, and the check itself, both go through the shell's
    // `checkVersion` — the startup notice asks the same question and the two must not be
    // able to disagree about the answer.
    S.checkVersion().then(function (body) {
      aboutVersion.textContent = versionLabel(body);
    });

    el.querySelector("#about-check").addEventListener("click", function () {
      var button = el.querySelector("#about-check");
      button.disabled = true;
      setPill(aboutState, "busy", "检查中…");
      aboutNote.textContent = "正在向官方站点查询最新版本…";

      S.checkVersion().then(function (body) {
        button.disabled = false;
        body = body || {};
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
      // The shell owns this now: the startup notice's "去下载新版本" and this button have
      // to fetch the same artifact for the same platform, so there is one URL builder and
      // one place to fix.
      S.openUpdateDownload();
    });

    function fill(loaded) {
      if (!loaded) return;
      base.value = loaded.api_base || "";
      proxy.value = loaded.proxy || "";
      sysProxy.checked = loaded.use_system_proxy !== false;
      cache.value = String(loaded.node_cache_seconds || 300);
      port.value = String(loaded.default_game_port || 25565);

      var blur = parseInt(loaded.background_blur, 10);
      if (!isFinite(blur)) blur = 0;
      bgBlur.value = String(blur);
      bgBlurValue.textContent = blur + " px";

      var darkness = parseInt(loaded.background_darkness, 10);
      if (!isFinite(darkness)) darkness = 62;
      bgDark.value = String(darkness);
      bgDarkValue.textContent = darkness + "%";

      bgPath.value = loaded.background_path || "";
      adoptCrop(loaded);

      where.innerHTML =
        "<dt>配置文件</dt><dd>" + esc(S.settingsMeta.path || "—") + "</dd>" +
        "<dt>平台 HTTP</dt><dd>" + esc(S.settingsMeta.http_backend || "—") + "</dd>";

      // Filling the form is also what applies it: the settings page is the only place
      // these three numbers are edited, and a form that showed one weight while the
      // window painted another is the bug this avoids.
      S.applyBackground(loaded).then(adoptBackground);
    }

    /** The sentence the shell refused a save with, or the honest fallback. */
    function errorText(body) {
      if (body && body.reason) return body.reason;
      if (body && body.error) return body.error;
      return "客户端没有响应";
    }

    S.loadSettings(false).then(fill);

    function markDirty() {
      setPill(state, "busy", "有未保存的修改");
    }

    [base, proxy, sysProxy, cache, port].forEach(function (input) {
      input.addEventListener("input", markDirty);
      input.addEventListener("change", markDirty);
    });

    /* ------------------------------------------------ 个性化：背景 */

    var bgDrop = el.querySelector("#set-bgdrop");
    var bgPath = el.querySelector("#set-bgpath");
    var bgFile = el.querySelector("#set-bgfile");
    var bgWhy = el.querySelector("#set-bgwhy");
    var bgState = el.querySelector("#set-bgstate");
    var bgBlur = el.querySelector("#set-bgblur");
    var bgBlurValue = el.querySelector("#set-bgblur-value");
    var bgDark = el.querySelector("#set-bgdark");
    var bgDarkValue = el.querySelector("#set-bgdark-value");
    var bgClear = el.querySelector("#set-bgclear");
    var bgCropField = el.querySelector("#set-bgcropfield");
    var bgStage = el.querySelector("#set-bgstage");
    var bgImage = el.querySelector("#set-bgimage");
    var bgBox = el.querySelector("#set-bgbox");
    var bgReadout = el.querySelector("#set-bgcrop-readout");
    var bgCropReset = el.querySelector("#set-bgcrop-reset");

    /*
     * The crop the user is dragging, as fractions of the picture.
     *
     * Page state until 保存设置 writes it: the rectangle previews live against the
     * real background behind this page, and 放弃修改 puts the saved one back. The
     * per-mille integers the shell stores are this multiplied by a thousand.
     */
    var crop = { x: 0, y: 0, w: 1, h: 1 };
    var MIN_CROP = 0.1;

    function clampFraction(value, low, high) {
      value = Number(value);
      if (!isFinite(value)) return low;
      return Math.max(low, Math.min(high, value));
    }

    /*
     * The crop, as the shell stores it: one nested object of four per-mille numbers.
     *
     * Nested rather than four flat fields because they are one value — a rectangle —
     * and `apply_json` reads them from inside that object, so a patch that carried
     * `background_crop_w` on its own would be silently ignored. That is not a
     * hypothetical: the first version of this page wrote exactly those flat keys, and
     * the crop it saved was the default 0,0,1000,1000 every time, which looks correct
     * until somebody drags the box.
     */
    function cropPatch() {
      return {
        background_crop: {
          x: Math.round(crop.x * 1000),
          y: Math.round(crop.y * 1000),
          w: Math.round(crop.w * 1000),
          h: Math.round(crop.h * 1000)
        }
      };
    }

    /*
     * What the two sliders and the box currently add up to.
     *
     * The picture in it is always the **saved** one, not the text in the path box:
     * that box is a field somebody is halfway through typing, and previewing a path
     * the shell has not accepted yet would draw last week's picture under this week's
     * filename. The path takes effect on 保存设置, like every other field here.
     */
    function draft() {
      var base = S.settings || {};
      return {
        background_path: base.background_path || "",
        background_blur: parseInt(bgBlur.value, 10) || 0,
        background_darkness: parseInt(bgDark.value, 10) || 0,
        background_crop: cropPatch().background_crop
      };
    }

    /** Where the picture is drawn inside the stage, in stage pixels. */
    function stageImageRect() {
      var stageWidth = bgStage.clientWidth;
      var stageHeight = bgStage.clientHeight;
      var natural = { w: bgImage.naturalWidth, h: bgImage.naturalHeight };
      if (!stageWidth || !stageHeight || !natural.w || !natural.h) return null;
      var scale = Math.min(stageWidth / natural.w, stageHeight / natural.h);
      var width = natural.w * scale;
      var height = natural.h * scale;
      return {
        left: (stageWidth - width) / 2,
        top: (stageHeight - height) / 2,
        width: width,
        height: height
      };
    }

    function paintCrop() {
      var rect = stageImageRect();
      if (!rect) return;
      bgBox.style.left = (rect.left + crop.x * rect.width) + "px";
      bgBox.style.top = (rect.top + crop.y * rect.height) + "px";
      bgBox.style.width = (crop.w * rect.width) + "px";
      bgBox.style.height = (crop.h * rect.height) + "px";
      bgReadout.textContent =
        Math.round(crop.w * bgImage.naturalWidth) + " × " +
        Math.round(crop.h * bgImage.naturalHeight) + " 像素";
    }

    /**
     * Everything the two sliders and the box move, applied to the page behind this
     * one — the preview *is* the real background, so there is no second rendering of
     * the picture that could disagree with it.
     */
    function preview() {
      S.previewBackground(draft());
      paintCrop();
    }

    /**
     * The crop tool exists only while there is a picture to crop.
     *
     * `S.background` is what the layer actually managed to put on screen, so this is
     * the one condition that cannot be wrong: a path the shell refused, a file that
     * went missing, or a format this browser will not decode all leave the tool
     * hidden rather than offering to crop a picture that is not there.
     */
    function syncCropTool() {
      var live = S.background;
      var usable = live.state === "ok" && !!live.url;
      bgCropField.hidden = !usable;
      if (!usable) {
        bgImage.removeAttribute("src");
        return;
      }
      if (bgImage.getAttribute("src") !== live.url) bgImage.setAttribute("src", live.url);
      // The rectangle needs the picture's natural size, which arrives with the load.
      if (bgImage.complete && bgImage.naturalWidth) paintCrop();
      else bgImage.onload = paintCrop;
    }

    /** The line under the path field: which picture is on screen, or why it is not. */
    function sayBackground() {
      var meta = S.settingsMeta.background || {};
      var live = S.background;
      var path = (S.settings && S.settings.background_path) || "";

      if (!path) {
        setPill(bgState, "down", "内置背景");
        bgWhy.innerHTML = "正在使用客户端自带的背景。把一张图片的完整路径填在上面，" +
          "或者把图片拖到这一行，就能换成它。";
        return;
      }
      if (live.state === "ok") {
        setPill(bgState, "ok", "自定义背景");
        var size = live.width ? "（" + live.width + " × " + live.height + "）" : "";
        bgWhy.innerHTML = "正在使用 " + esc(meta.path || path) + size +
          "。原图被移动或删除后，会自动退回内置背景。";
        return;
      }
      setPill(bgState, "down", "已退回内置背景");
      bgWhy.innerHTML = "这张图片现在读不到，已经退回内置背景：" +
        esc(live.reason || meta.reason || "原因未知");
    }

    function adoptBackground() {
      syncCropTool();
      sayBackground();
    }

    function adoptCrop(loaded) {
      var fraction = S.cropFraction(loaded);
      crop = { x: fraction.x, y: fraction.y, w: fraction.w, h: fraction.h };
    }

    /*
     * A dropped or picked file can only ever be a hint.
     *
     * The browser hands over a name, a size and a timestamp and nothing else — the
     * path is a security boundary it does not cross, and no amount of asking changes
     * that. So this fills in the file name and says what is missing, instead of
     * pretending to accept an upload and quietly copying the picture somewhere.
     *
     * The two checks here are the ones a `File` can answer: its size, and the name it
     * arrived with. What the file *is* gets decided by the shell, from its first
     * bytes, and refused there with a sentence that names what is accepted.
     */
    function prefillFromFile(file) {
      if (!file) return;
      if (/\.gif$/i.test(file.name || "")) {
        S.toast("不支持 GIF：会动的图片不能做背景", "warn");
        return;
      }
      if (file.size > BACKGROUND_MAX_MB * 1024 * 1024) {
        S.toast("这张图片有 " + (file.size / 1048576).toFixed(1) + " MB，超过了 " +
          BACKGROUND_MAX_MB + " MB 上限", "warn");
        return;
      }
      var value = bgPath.value.trim();
      if (/[\\/]$/.test(value)) {
        value = value + file.name;
      } else {
        var cut = Math.max(value.lastIndexOf("\\"), value.lastIndexOf("/"));
        value = cut >= 0 ? value.slice(0, cut + 1) + file.name : file.name;
      }
      bgPath.value = value;
      markDirty();
      S.toast("已填上文件名。浏览器不会把完整路径交给我们，请把目录补全", "warn");
    }

    // The row is the drop target, so the whole field lights up rather than a strip of
    // it. `dragleave` fires on the children too — without the containment test the
    // highlight flickers as the pointer crosses the input inside the row.
    ["dragenter", "dragover"].forEach(function (name) {
      bgDrop.addEventListener(name, function (event) {
        event.preventDefault();
        bgDrop.dataset.over = "1";
      });
    });
    bgDrop.addEventListener("dragleave", function (event) {
      if (event.relatedTarget && bgDrop.contains(event.relatedTarget)) return;
      bgDrop.dataset.over = "0";
    });
    bgDrop.addEventListener("drop", function (event) {
      event.preventDefault();
      bgDrop.dataset.over = "0";
      var files = event.dataTransfer && event.dataTransfer.files;
      if (files && files.length) prefillFromFile(files[0]);
    });

    el.querySelector("#set-bgbrowse").addEventListener("click", function () {
      bgFile.click();
    });
    bgFile.addEventListener("change", function () {
      if (bgFile.files && bgFile.files.length) prefillFromFile(bgFile.files[0]);
      // Cleared so that picking the same file twice fires `change` twice: without
      // this, a second attempt at a file the user already chose does nothing at all.
      bgFile.value = "";
    });

    bgPath.addEventListener("input", markDirty);
    bgPath.addEventListener("change", markDirty);

    bgBlur.addEventListener("input", function () {
      bgBlurValue.textContent = bgBlur.value + " px";
      markDirty();
      preview();
    });
    bgDark.addEventListener("input", function () {
      bgDarkValue.textContent = bgDark.value + "%";
      markDirty();
      preview();
    });

    /**
     * Move or resize the rectangle, from a drag in stage coordinates.
     *
     * `from` is where the gesture started, so every move is computed against the
     * original rectangle rather than against the previous move: accumulating deltas
     * is how a drag drifts when a pointer event is dropped.
     */
    function applyDrag(mode, from, dx, dy) {
      if (mode === "move") {
        crop = {
          x: clampFraction(from.x + dx, 0, 1 - from.w),
          y: clampFraction(from.y + dy, 0, 1 - from.h),
          w: from.w,
          h: from.h
        };
        return;
      }
      var left = from.x;
      var top = from.y;
      var right = from.x + from.w;
      var bottom = from.y + from.h;
      if (mode.indexOf("w") >= 0) left = clampFraction(from.x + dx, 0, right - MIN_CROP);
      if (mode.indexOf("e") >= 0) right = clampFraction(right + dx, left + MIN_CROP, 1);
      if (mode.indexOf("n") >= 0) top = clampFraction(from.y + dy, 0, bottom - MIN_CROP);
      if (mode.indexOf("s") >= 0) bottom = clampFraction(bottom + dy, top + MIN_CROP, 1);
      crop = { x: left, y: top, w: right - left, h: bottom - top };
    }

    /*
     * Pointer capture, so the drag keeps working when the pointer leaves the box —
     * which it does constantly, since the handles sit on the box's own edge. Every
     * listener is on the box and is removed when the gesture ends, and none of them is
     * on `document`: a page that adds document listeners has to remove them again on
     * `destroy`, and the box is enough.
     */
    bgBox.addEventListener("pointerdown", function (event) {
      if (event.button !== 0) return;
      var rect = stageImageRect();
      if (!rect) return;
      event.preventDefault();

      var mode = (event.target && event.target.dataset && event.target.dataset.corner) || "move";
      var startX = event.clientX;
      var startY = event.clientY;
      var from = { x: crop.x, y: crop.y, w: crop.w, h: crop.h };
      var box = bgBox;
      try {
        box.setPointerCapture(event.pointerId);
      } catch (err) {
        // An old browser, or a pointer the capture refused: the gesture still works
        // while the pointer stays over the box, and the listeners are cleaned up the
        // moment it is released.
      }

      function onMove(move) {
        applyDrag(mode, from, (move.clientX - startX) / rect.width, (move.clientY - startY) / rect.height);
        markDirty();
        preview();
      }
      function onEnd() {
        box.removeEventListener("pointermove", onMove);
        box.removeEventListener("pointerup", onEnd);
        box.removeEventListener("pointercancel", onEnd);
      }
      box.addEventListener("pointermove", onMove);
      box.addEventListener("pointerup", onEnd);
      box.addEventListener("pointercancel", onEnd);
    });

    bgCropReset.addEventListener("click", function () {
      crop = { x: 0, y: 0, w: 1, h: 1 };
      markDirty();
      preview();
    });

    bgClear.addEventListener("click", function () {
      S.saveSettings({ background_path: "" }).then(function (result) {
        if (!result.ok) {
          S.toast("恢复失败：" + errorText(result.body), "warn");
          return;
        }
        bgPath.value = "";
        setPill(state, "ok", "已保存");
        S.applyBackground(S.settings).then(adoptBackground);
        S.toast("已经换回内置背景");
      });
    });

    /**
     * Put a newly chosen picture's own idea of 背景变暗 on the slider, and save it.
     *
     * Measured in the page from the picture's pixels (`suggestDarkness`), because how
     * heavy the wash has to be is a property of the picture: a dark wallpaper needs
     * almost none, a white one needs most of it. The save is tiny and it is worth it —
     * the alternative is every user starting at 62% and dragging a slider to find out
     * what their own wallpaper wanted.
     */
    function seedDarkness() {
      S.suggestDarkness(S.background.url).then(function (value) {
        if (value === null) return;
        bgDark.value = String(value);
        bgDarkValue.textContent = value + "%";
        S.saveSettings({ background_darkness: value }).then(function () {
          S.previewBackground(S.settings);
          S.toast("已按这张图的亮度把「背景变暗」设为 " + value + "%");
        });
      });
    }

    el.querySelector("#set-save").addEventListener("click", function () {
      var previousPath = (S.settings && S.settings.background_path) || "";
      var patch = {
        api_base: base.value.trim(),
        proxy: proxy.value.trim(),
        use_system_proxy: sysProxy.checked,
        node_cache_seconds: parseInt(cache.value, 10) || 300,
        default_game_port: parseInt(port.value, 10) || 25565,
        background_path: bgPath.value.trim(),
        background_blur: parseInt(bgBlur.value, 10) || 0,
        background_darkness: parseInt(bgDark.value, 10) || 0
      };
      patch.background_crop = cropPatch().background_crop;
      S.saveSettings(patch).then(function (result) {
        var body = result.body || {};
        if (result.ok && body.settings) {
          var changed = (body.settings.background_path || "") !== previousPath;
          fill(body.settings);
          setPill(state, "ok", "已保存");
          S.toast("设置已保存到 " + (body.settings.path || "配置文件"));
          if (changed) {
            S.applyBackground(body.settings).then(function () {
              adoptBackground();
              if (S.background.state === "ok") seedDarkness();
            });
          }
        } else {
          /*
           * A refused background is the interesting failure here, and the shell says
           * which one it was — 找不到这个文件 / 不支持 GIF / 超过 20 MB — so it goes on
           * screen where the field is rather than into a toast that disappears.
           */
          setPill(state, "down", "保存失败");
          var why = errorText(body);
          S.toast("保存失败：" + why, "warn");
          bgWhy.innerHTML = esc(why);
          // Nothing was written, so the live preview goes back to what is stored —
          // but only the *layer*: `adoptBackground` would also rewrite the line above
          // with the current state's sentence, and the sentence the user needs right
          // now is the one saying why their path was refused.
          S.applyBackground(S.settings).then(syncCropTool);
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

  /* ------------------------------------------------------------------- 帮助 */

  /*
   * 帮助 is one page with a list of questions on it, and this is the first question.
   *
   * It is part of the client rather than a link to a website for one reason: the question
   * ("what is this number you are asking me for?") arrives while somebody is half-way
   * through opening a room, and sending them to a browser tab loses the room. The pictures
   * are the client's own assets, so the page also works with no network beyond loopback.
   *
   * The list is data because a second question is already known to be coming: one more
   * entry plus one more article function is the whole change. It only renders as a row of
   * chips once there is more than one question — a tab bar with a single tab is furniture.
   */
  var HELP_TOPICS = [
    {
      slug: "port",
      name: "什么是游戏端口",
      blurb: "那个数字是什么、在哪看、填到哪",
      sources: helpSources,
      article: helpPortArticle,
    },
    {
      slug: "join",
      name: "朋友怎么加入房间",
      blurb: "把地址发过去之后，他那边要做的三步",
      sources: helpSources,
      article: helpJoinArticle,
    },
    {
      slug: "trouble",
      name: "朋友连不上怎么办",
      blurb: "对着屏幕上的报错找，从最常见的那条往下看",
      sources: caseSources,
      article: helpTroubleArticle,
    },
  ];

  /**
   * Where the screenshots come from, said before the first one rather than under it.
   *
   * A picture of somebody else's game is only useful if the reader can tell whether their
   * own game looks the same, and the answer here is "yes, if your version and mod list
   * match" — which is also the first thing that goes wrong when a friend cannot join. So
   * this line is doing double duty, and it is not a footnote.
   */
  function helpSources() {
    return '<p class="help-sources">图片均来自 <b>Minecraft Java 版 26.3</b>，搭配模组 ' +
      "<b>LAN World Plug-n-Play (mcwifipnp)</b>（用来关闭正版验证）。" +
      "版本或模组不同的话，按钮位置可能略有出入。</p>";
  }

  /**
   * The troubleshooting page's screenshots, and a different sentence about them than the
   * one above: those four were taken for the 26.3 walkthrough, while the case screenshots
   * were collected over months and some of them are visibly older — one is 1.21.11, two are
   * phone-client layouts. Saying so is the honest version, and it also tells the reader
   * what to compare: the error *text* is the same on every version, the window around it is
   * not.
   */
  function caseSources() {
    return '<p class="help-sources">下面这些截图是从历次反馈里攒的，所以版本各不相同' +
      "（能看出 1.21.11 和手机版的界面）。<b>报错文字本身没变过</b>，" +
      "对着文字找就行，窗口和按钮会和新版本有些差别。</p>";
  }

  /**
   * One screenshot with its caption.
   *
   * The caption doubles as the `alt`, which is not laziness: the picture and the sentence
   * say the same thing on purpose — a screenshot in a guide is there to be *recognised*,
   * and the text beside it is what says it in words.
   */
  function helpShot(file, caption, extraClass) {
    return '<figure class="help-shot' + (extraClass ? " " + extraClass : "") + '">' +
      '<img src="' + esc(S.assetUrl("asset/" + file)) + '" alt="' + esc(caption) +
      '" loading="lazy">' +
      "<figcaption>" + esc(caption) + "</figcaption></figure>";
  }

  /** One numbered step of a procedure. */
  function helpStep(n, html) {
    return '<p class="help-step"><span class="help-step-num">' + n + "</span>" +
      "<span>" + html + "</span></p>";
  }

  /**
   * One question and its answer, for the page whose whole shape is a list of them.
   *
   * `shots` is a list of `[file, caption]` and is optional. A case is much easier to
   * recognise than to read about, and the reader has their own screen in front of them —
   * so every case in the troubleshooting list can carry the error it is about.
   */
  function helpFaq(question, answer, shots) {
    return '<div class="help-faq-item"><p class="help-faq-q">' + question + "</p>" +
      answer +
      (shots || []).map(function (shot) {
        return helpShot(shot[0], shot[1], "help-shot--case");
      }).join("") +
      "</div>";
  }

  /** The address, in the article's own words: the thing being copied has two halves. */
  function addressSample() {
    return '<code class="help-code">cd.hongshi.site:27913</code>';
  }

  /**
   * 什么是游戏端口 — the answer, in three parts: what it is, where to see yours, where to
   * put it. The middle part is the client's own three-step procedure (ESC → 世界选项 →
   * 应用更改) with a screenshot per step, because that is the part somebody is following
   * with the game open on the other monitor.
   */
  function helpPortArticle() {
    return section("端口是游戏开的一扇门", "globe",
      "<p>开了局域网，Minecraft 就在你这台电脑上开了一扇门，" +
      "<b>端口就是这扇门的门牌号</b>。</p>" +
      "<p>它和 IP 不是一回事：IP 是你这栋楼，端口是楼里的哪一间。同一条网线上同时住着网页、" +
      "下载器和游戏，靠的就是门牌号不同。</p>" +
      "<p>朋友要进门，只需要知道门牌号；但公网上的人连你的楼在哪儿都看不到 —— 这也是为什么" +
      "自己开局域网，只有同一条 Wi-Fi 下的人进得来。<b>红石联机做的事，就是把这扇门接到" +
      "中转服务器上</b>，再给你一个能直接发出去的地址：路由器不用碰，端口映射也不用管。</p>",

      "下面两步在游戏里完成，第三步才回到红石联机。") +

      section("怎么看自己的端口", "console",
        '<p class="help-lead">以《我的世界》Java 版为例，三步就能看到它。</p>' +
        helpStep(1, "在游戏里按 <kbd>ESC</kbd> 打开游戏菜单，点「<b>世界选项…</b>」。") +
        helpShot("help-port-menu.webp", "ESC 菜单：点这里进世界选项") +
        helpStep(2, "把「多人游戏」点成「<b>局域网</b>」。下面的<b>端口号</b>就是门牌号，" +
          "默认 25565，想换一个也行。") +
        helpShot("help-port-lan.webp", "世界选项 → 多人游戏：端口号写在下面") +
        helpStep(3, "按底部的「<b>应用更改</b>」，聊天框里会冒出一行字，方括号里的数字就是它。") +
        helpShot("help-port-chat.webp", "聊天框：端口号为 [25565]") +
        '<p class="help-tip">紧跟着的那行「无法转发端口 … 路由器上未启用 UPnP」' +
        "<b>不用管它</b>。那是游戏在试着自己打洞，家用路由器上失败是常态；" +
        "红石联机不走 UPnP，门怎么开出去是交给中转服务器的。</p>",

        // The middle section ends on the answer, so its note is that answer in one line
        // rather than another sentence of instruction.
        "看到方括号里那个数字，就已经找到了。") +

      section("填进红石联机", "play",
        "<p>回到「联机」页，把这个数字填进<b>本地游戏端口</b>。红石一般已经替你探到了，" +
        "你只要核对一眼；探不到、或者你已经换过端口，就自己改。</p>" +
        "<p>填错了它就会去敲一扇不存在的门，朋友那边会一直卡在「正在连接」——" +
        "这种情况先回来看看这个数字对不对。</p>" +
        "<p>然后按「开启房间」，把弹出来的地址发给朋友，他填进游戏里就能进来了。</p>" +
        '<p class="help-cta"><a class="btn btn--primary" href="/connect">' +
        icon("play", "icon--sm") + "去联机页开启房间</a></p>");
  }

  /**
   * 朋友怎么加入房间 — the other half of the room flow, and the half that used to be a
   * sentence in a dialog: "把地址发给他". The person who opens the room never sees these
   * three screens; the person they send the address to does, so the article is written for
   * the friend, and the host forwards it.
   */
  function helpJoinArticle() {
    return section("三步进房间", "people",
      "<p>红石给你的那行地址是可以直接发出去的。朋友那边打开游戏，做三件事：</p>" +
      helpStep(1, "进游戏，点「<b>多人游戏</b>」。版本和模组要和房主一致 —— " +
        "左下角写着版本号和模组数量，对不上就会在加载到一半时被踢出来。") +
      helpShot("help-join-main.webp", "主界面：左下角写着版本与模组数，点「多人游戏」") +
      helpStep(2, "等它扫完局域网。这里通常什么都扫不到 —— 那是正常的，红石的房间不在局域网里，" +
        "所以直接点右下角的「<b>直接连接</b>」。") +
      helpShot("help-join-multiplayer.webp", "多人游戏界面：点右下角的「直接连接」") +
      helpStep(3, "把地址<b>整行</b>粘进「服务器地址」，点「<b>加入服务器</b>」。") +
      helpShot("help-join-direct.webp", "直接连接：粘贴地址，点「加入服务器」") +

      '<p class="help-tip">地址长这样：' + addressSample() + "。冒号前面是中转服务器的地址，" +
      "冒号后面是这次房间的端口 —— <b>少一半都进不来</b>。所以别手打，整行复制过去。</p>",

      "进不去的话，先看「朋友连不上怎么办」。") +

      section("房间开着的这一段时间", "link",
        "<p>房间只在<b>红石显示着地址</b>的时候存在。点了「关闭房间」，或者房主把客户端关掉，" +
        "这行地址就失效了，朋友会卡在「正在连接」。</p>" +
        "<p>房主那边退出世界、电脑休眠、断网，也会掉线：隧道还开着，但门后面没有人了。" +
        "重新开一次房间，地址会换一个新的，记得重新发。</p>" +
        '<p class="help-cta"><a class="btn btn--ghost" href="/help/trouble">' +
        icon("warn", "icon--sm") + "常见问题：朋友连不上怎么办？</a></p>");
  }

  /**
   * 朋友连不上怎么办 — written backwards from the screen.
   *
   * Somebody opens this page with a game window behind it showing a sentence, so every case
   * leads with **that sentence verbatim**, in the monospace face the game uses, and only
   * then says what it means. The order is by frequency, and the one case that needs the
   * *host* to act is marked as the fiddly one, because the friend reading this cannot fix
   * it alone.
   *
   * The cases themselves come from the support deck the team kept before this page existed
   * (`常见问题.pdf`); its screenshots are in `web/asset/help-trouble-*.webp`. Two things in
   * it were out of date and are fixed here rather than carried over: port forwarding is not
   * something this product needs (relaying is the whole point), and the "无效会话" and
   * "无效的玩家档案公钥签名" are one cause with two wordings.
   */
  function helpTroubleArticle() {
    return section("先自检三件事", "check",
      "<p>这三条里有一条对不上，多半就是它了；三条都对得上，再往下按报错找。</p>" +
      '<ol class="help-check">' +
      "<li>房主那边的红石<b>还显示着那行地址</b>吗？如果显示的是「开启房间」按钮，说明房间已经关了。</li>" +
      "<li>两个人的<b>版本号和模组数量一样</b>吗？主界面左下角就写着。</li>" +
      "<li>地址是<b>整行复制</b>过去的吗？" + addressSample() + "，两段都不能少。</li>" +
      "</ol>",

      "顺带说一句：红石联机的地址只在房间开着的时候有效。") +

      section("对着报错找", "warn",
        helpFaq(
          '<code class="help-code">无效会话</code> / ' +
          '<code class="help-code">无效的玩家档案公钥签名</code>',
          "<p>两句是同一个原因：<b>双方至少有一方是离线用户</b>，而开房间那一侧还开着正版验证，" +
          "登录的时候对不上。</p>" +
          '<ul><li>装 <b>LAN World Plug-n-Play (mcwifipnp)</b> 或 <b>LAN Server Properties</b> ' +
          "把正版验证关掉 —— <b>房主必须装</b>（要关的是开房间那一侧），朋友也装上更稳；</li>" +
          "<li>关完<b>重启一次世界</b>，再重新开局域网；</li>" +
          "<li>两边都是正版账号却还报这句：重启游戏，在启动器里重新登录一次。</li></ul>",
          [["help-trouble-signature.webp", "连接已丢失：无效的玩家档案公钥签名"]]) +

        helpFaq(
          "服务器发送含有未知键的注册表（Registry / Mod Mismatch）",
          "<p>两边的<b>模组不同步</b>：房主装了某个模组你没有，或者版本对不上。" +
          "报错里会点名是哪个模组 —— 截图里是 " +
          '<code class="help-code">redstoneonline:example_item</code>。</p>' +
          '<ul><li>照着报错里的模组名去补，两边装成一模一样；</li>' +
          "<li>最省事的不是一个个补：让房主把 <b>mods 文件夹</b>（或者整个整合包）打包发给你；</li>" +
          "<li><b>版本号也要一致</b>（26.3 对 26.3）。小地图、光影这类客户端模组影响小，" +
          "内容类、服务端模组必须一样。</li></ul>",
          [["help-trouble-registry.webp", "连接已丢失：服务器发送含有未知键的注册表"]]) +

        helpFaq(
          '<code class="help-code">Unknown host</code>（未知主机）',
          "<p>地址写错了。注意<b>这不是端口错</b> —— 游戏连域名都没解析出来，" +
          "所以<b>和房主那边的状态无关</b>。</p>" +
          '<ul><li>常见的错法：把冒号打成了中文「：」、混进全角字符、字母看错（<code class="help-code">l</code> 和 ' +
          '<code class="help-code">1</code>、<code class="help-code">o</code> 和 ' +
          '<code class="help-code">0</code>）、漏字符、末尾多了空格、只复制了冒号前面一半；</li>' +
          "<li>整行复制粘贴，别手打，粘完看一眼冒号前后；</li>" +
          "<li>换了地址还这样，用手机热点试一次（本机 DNS 的问题，少见）。</li></ul>",
          [["help-trouble-unknownhost.webp", "无法连接至服务器：Unknown host"]]) +

        helpFaq(
          '<code class="help-code">Connection refused: getsockopt</code> / ' +
          '<code class="help-code">连接中断</code>' +
          '<span class="help-tag">较麻烦</span>',
          "<p>地址是<b>对的</b>，但那扇门后面没有人。按这个顺序查，四条都在房主那边：</p>" +
          "<ol><li>红石上<b>还显示着地址</b>吗？显示「开启房间」= 房间关了，或者客户端退了；</li>" +
          "<li>房主的<b>游戏世界还开着</b>吗？退出世界 = 门后面没人；</li>" +
          "<li>红石里填的<b>本地游戏端口</b>，和游戏里报出来的端口对得上吗？" +
          "换过世界、重开过局域网，这个数字就会变；</li>" +
          "<li>房主电脑的<b>防火墙</b>或安全软件拦了红石或游戏 —— 最少见，但确实有。</li></ol>" +
          '<p class="help-tip"><b>红石联机不需要端口映射。</b>' +
          "网上那些让你去路由器上开端口的教程，这里用不上 —— 门是从中转服务器开出去的。" +
          "另外房主<b>重开一次房间会换一个新地址</b>，记得让他重新发。</p>",
          [["help-trouble-refused.webp", "无法连接至服务器：Connection refused: getsockopt"],
           ["help-trouble-lost.webp", "进去一会儿之后：连接中断"]]) +

        helpFaq(
          "暂时无法连接到身份验证服务器 / 身份验证失败",
          "<p>微软的验证服务器连不上，或者你的网络到它不通。<b>这一条跟红石联机、跟游戏本身都没有关系</b>，" +
          "大概率也不是你的问题。</p>" +
          '<ul><li>先问一句是不是只有你一个人这样：大家都不行，就是微软那边在抽风，等一会儿再来；</li>' +
          "<li>只有你不行，就是你的网络到验证服务器的链路问题，开加速器或者换个网络再试。</li></ul>",
          [["help-trouble-auth.webp", "无法连接至服务器：登录失败，暂时无法连接到身份验证服务器"]]) +

        "",

        "报错文字和上面任何一条都不像的话，往下看最后一段。") +

      section("还是不行", "people",
        "<p>上面的清单里没有你的报错，或者照着试完还是进不去 —— 来官方 Q 群问一句。</p>" +
        '<p class="help-cta">' +
        '<button type="button" class="btn btn--primary" data-copy="497060189">' +
        icon("copy", "icon--sm") + "复制群号 497060189</button>" +
        '<a class="btn btn--ghost" href="/connect">' +
        icon("play", "icon--sm") + "回到联机页</a></p>" +
        "<p>顺带带上这三样，基本当场就能定位：</p>" +
        "<ul>" +
        "<li>屏幕上的<b>报错截图</b>（房主和朋友两边都截一张）；</li>" +
        "<li>两边的<b>版本号和模组数量</b>（主界面左下角）；</li>" +
        "<li>红石那边是<b>显示着地址</b>，还是<b>「开启房间」按钮</b>。</li>" +
        "</ul>");
  }

  /**
   * One help page, rendered for one question.
   *
   * `/help` and `/help/<slug>` are the same document (the shell serves every route the
   * same way), so both land here: with nothing to name, the first question is the one
   * shown, which is also what `/help` should do once there are five.
   */
  function helpPage(el, slug) {
    var topic = HELP_TOPICS.filter(function (entry) { return entry.slug === slug; })[0] ||
      HELP_TOPICS[0];

    var topics = HELP_TOPICS.length > 1
      ? '<nav class="help-topics" aria-label="帮助目录">' +
        HELP_TOPICS.map(function (entry) {
          var here = entry === topic;
          return '<a class="help-topic" href="/help/' + entry.slug + '"' +
            (here ? ' aria-current="page"' : "") + ">" + esc(entry.name) + "</a>";
        }).join("") + "</nav>"
      : "";

    // `sources` marks the questions whose screenshots need saying where they came from —
    // and it is said *before* the first one, because "does my game look like this?" is the
    // question the reader has while looking at somebody else's menu bar.
    el.innerHTML = topics + pageHead(topic.name, esc(topic.blurb)) +
      (topic.sources ? topic.sources() : "") +
      '<article class="help-article">' + topic.article() + "</article>";

    /*
     * The one control in 帮助 that does something: the Q group number.
     *
     * Reading six digits off a screen and typing them into a phone is exactly where people
     * give up, so it is a button that copies. Bound to the element rather than delegated —
     * it lives inside `#page`, which the router empties on the way out, so the listener
     * leaves with the markup it belongs to.
     */
    el.querySelectorAll("[data-copy]").forEach(function (button) {
      button.addEventListener("click", function () {
        S.copyText(button.dataset.copy, "群号");
      });
    });

    return { title: topic.name };
  }

  /* ---------------------------------------------------------------- register */

  // `/help` is the page and `/help/<slug>` is a question in it. Both are registered, so
  // neither falls through to the "not built yet" placeholder.
  S.register("help", function (el) { return helpPage(el, null); });
  S.register("help/port", function (el) { return helpPage(el, "port"); });
  S.register("help/join", function (el) { return helpPage(el, "join"); });
  S.register("help/trouble", function (el) { return helpPage(el, "trouble"); });
  S.register("home", homePage);
  S.register("connect", connectPage);
  S.register("settings", settingsPage);
  S.register("soon", soonPage);

  // The routes exist now, so the shell can draw one. `app.js` owns the boot
  // sequence and calls this; without it nothing would ever render, which is why
  // the shell also has a watchdog for this script not arriving at all.
  S.ready();
})();
