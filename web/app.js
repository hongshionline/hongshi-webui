/*
 * hongshi shell — runtime.
 *
 * Owns everything a page should not have to know about: the API wrapper, the
 * router, the log terminal, and the settings cache. `pages.js` only draws.
 *
 * Progressive enhancement is still the rule: with this script gone the shell has
 * already told the user on its console what the URL is, and the sidebar is plain
 * links. Nothing here is the only way to learn something.
 */

(function () {
  "use strict";

  var SIDE_KEY = "hongshi.shell.side";
  var PAGE_POLL_MS = 3000;
  /* Drives the 创建于 / 已运行 counters. Ten seconds, not one: they are formatted
     coarsely ("2 分钟前"), and a per-second timer is a per-second wake-up for a
     number that reads the same either way. */
  var TICK_MS = 10000;
  /* The kernel is a local child process, so a poll is a loopback request with no
     upstream cost — but it is still a wake-up, so it runs at 1 Hz only while a tunnel
     is actually alive, 0.2 Hz while an open panel waits for one, and not at all
     otherwise. */
  var KERNEL_POLL_MS = 1000;
  var KERNEL_IDLE_POLL_MS = 5000;

  /* -------------------------------------------------------------------- api */

  /*
   * No session token.
   *
   * There used to be one: 32 hex characters kept in `sessionStorage`, stripped out
   * of the address bar on load, and appended to every request. It bought a boundary
   * against other programs on the machine and cost the user a page that died on
   * reload — a refresh re-requests `/` with no query string, so the shell answered
   * its own interface with 403 before a single line of this file ran. The server
   * refuses a request by where it came from now (loopback `Host`, same-origin
   * `Origin`) rather than by what it carries, which needs nothing from here.
   */
  function api(path, options) {
    var init = options || {};
    init.cache = "no-store";
    return window.fetch(path, init);
  }

  function apiJson(path, options) {
    return api(path, options).then(function (response) {
      return response.json().then(
        function (body) { return { ok: response.ok, status: response.status, body: body }; },
        function () { return { ok: response.ok, status: response.status, body: null }; }
      );
    });
  }

  function apiSend(path, payload, method) {
    return apiJson(path, {
      method: method || "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(payload)
    });
  }

  /* --------------------------------------------------------------- terminal */

  var MAX_TERM_LINES = 600;
  var termEl = document.getElementById("term");
  var lastLogSignature = null;
  var lastLogRepeat = 0;

  function pad(value, width) {
    var text = String(value);
    while (text.length < width) text = "0" + text;
    return text;
  }

  function clock() {
    var now = new Date();
    return pad(now.getHours(), 2) + ":" + pad(now.getMinutes(), 2) + ":" + pad(now.getSeconds(), 2);
  }

  function isScrolledToBottom(el) {
    return el.scrollHeight - el.scrollTop - el.clientHeight < 40;
  }

  function termWrite(text, level) {
    if (!termEl) return;
    var levelName = level || "info";
    var signature = levelName + "\u0000" + text;

    // The same line twice in a row is collapsed rather than repeated: the log gets
    // a "×N" counter, which is what a terminal does and what keeps a retry loop
    // from filling the screen.
    if (signature === lastLogSignature && termEl.lastElementChild) {
      lastLogRepeat += 1;
      var counter = termEl.lastElementChild.querySelector(".term-count");
      if (!counter) {
        counter = document.createElement("span");
        counter.className = "term-count";
        counter.style.color = "#55606d";
        termEl.lastElementChild.querySelector(".term-body").appendChild(counter);
      }
      counter.textContent = "  ×" + (lastLogRepeat + 1);
      if (isScrolledToBottom(termEl)) termEl.scrollTop = termEl.scrollHeight;
      return;
    }

    lastLogSignature = signature;
    lastLogRepeat = 0;

    var line = document.createElement("div");
    line.className = "term-line";
    line.dataset.level = levelName;

    var time = document.createElement("span");
    time.className = "term-time";
    time.textContent = clock();

    var body = document.createElement("span");
    body.className = "term-body";
    body.textContent = text;

    line.appendChild(time);
    line.appendChild(body);
    termEl.appendChild(line);

    while (termEl.children.length > MAX_TERM_LINES) {
      termEl.removeChild(termEl.firstElementChild);
    }
    if (isScrolledToBottom(termEl)) termEl.scrollTop = termEl.scrollHeight;
  }

  function termClear() {
    if (!termEl) return;
    termEl.textContent = "";
    lastLogSignature = null;
    lastLogRepeat = 0;
  }

  var drawer = document.getElementById("drawer");
  var appEl = document.getElementById("app");

  function drawerOpen(open) {
    if (!drawer || !appEl) return;
    drawer.hidden = !open;
    appEl.dataset.drawer = open ? "open" : "closed";

    // The switch has to say which way it goes, and it is the only control that is
    // present whether the panel is open or closed.
    var toggle = document.getElementById("log-toggle");
    if (toggle) {
      toggle.setAttribute("aria-expanded", open ? "true" : "false");
      toggle.setAttribute("aria-label", open ? "收起日志" : "展开日志");
      toggle.setAttribute("title", open ? "收起日志" : "日志");
    }

    // The kernel's log is only worth polling while something is worth watching: a
    // live kernel, or an open panel waiting for one.
    if (open) kernelStore.watch(true);
    else kernelStore.watch(!!(kernelStore.info && kernelStore.info.running));
  }

  function drawerIsOpen() {
    return drawer && !drawer.hidden;
  }

  /* ------------------------------------------------------------------ nodes */

  /*
   * The relay list and its measurements, held for the life of the tab.
   *
   * This used to live inside the 联机 page, which meant every visit re-read the list
   * *and* re-probed every node — a round of ICMP/TCP per relay on a page you might
   * open four times in a minute, for a list that changes when the operators add a
   * machine and not otherwise. The client now reads it once at startup and keeps the
   * measurements; every page renders from the same snapshot. The two buttons on 联机
   * are what ask for fresh data, and the shell still caches the upstream read for
   * `node_cache_seconds` so a forced refresh does not hammer the site.
   */
  var nodeStore = {
    nodes: [],
    state: "idle", // idle | loading | ok | missing | error
    reason: "",
    source: "",
    probedAt: null,
    probing: false,
    loadedOnce: false,
    pending: null,
    subscribers: [],

    subscribe: function (listener) {
      var self = this;
      this.subscribers.push(listener);
      return function () {
        var index = self.subscribers.indexOf(listener);
        if (index >= 0) self.subscribers.splice(index, 1);
      };
    },

    publish: function () {
      // Copied before iterating: a listener is free to unsubscribe itself.
      var listeners = this.subscribers.slice();
      for (var i = 0; i < listeners.length; i++) {
        try {
          listeners[i](this.nodes);
        } catch (err) {
          /* one section failing must not stop the others from redrawing */
        }
      }
    },

    /** The list, read once. `force` re-reads it from the site. */
    load: function (force) {
      var self = this;
      if (this.loadedOnce && !force) return Promise.resolve(this.nodes);
      if (this.pending) return this.pending;

      this.state = "loading";
      this.publish();

      this.pending = apiJson("/api/nodes" + (force ? "?refresh=1" : ""))
        .then(function (result) {
          var body = (result && result.body) || {};
          self.pending = null;
          self.loadedOnce = true;
          self.source = body.source || "";

          if (body.state !== "ok" || !Array.isArray(body.nodes) || !body.nodes.length) {
            self.nodes = [];
            self.state = body.state === "missing" ? "missing" : "error";
            self.reason = body.reason || "";
            self.publish();
            return self.nodes;
          }

          // Keep the measurements already taken for hosts still on the list, so a
          // refresh does not blank the latency column while the new probe runs.
          var measured = {};
          self.nodes.forEach(function (node) { measured[node.host] = node; });

          self.nodes = body.nodes.map(function (node) {
            var old = measured[node.host];
            if (old && typeof old.latency_ms === "number") {
              node.latency_ms = old.latency_ms;
              node.probe_state = old.probe_state;
              node.probe_method = old.probe_method;
              node.probe_reason = old.probe_reason;
            }
            return node;
          });
          self.state = "ok";
          self.publish();
          return self.nodes;
        })
        .catch(function (err) {
          self.pending = null;
          self.loadedOnce = true;
          self.nodes = [];
          self.state = "error";
          self.reason = String(err);
          self.publish();
          return self.nodes;
        });

      return this.pending;
    },

    /** Measure every node's latency. Only a manual run complains out loud. */
    probe: function (manual) {
      var self = this;
      if (this.probing) return Promise.resolve(this.nodes);
      if (!this.nodes.length) {
        if (manual) toast("还没有节点可以测速", "warn");
        return Promise.resolve(this.nodes);
      }

      this.probing = true;
      if (manual) termWrite("正在探测 " + this.nodes.length + " 个节点的控制端口…", "ident");

      var hosts = this.nodes.map(function (node) { return node.host; });
      return apiSend("/api/probe", { hosts: hosts }, "POST").then(function (result) {
        self.probing = false;
        var body = (result && result.body) || {};
        if (!result.ok || !Array.isArray(body.results)) {
          if (manual) toast("测速失败：" + (body.reason || "客户端没有响应"), "warn");
          return self.nodes;
        }

        var byHost = {};
        body.results.forEach(function (entry) { byHost[entry.host] = entry; });

        self.nodes = self.nodes.map(function (node) {
          var entry = byHost[node.host] || {};
          return Object.assign({}, node, {
            latency_ms: typeof entry.latency_ms === "number" ? entry.latency_ms : null,
            probe_state: entry.state || "unknown",
            probe_method: entry.method || "none",
            probe_reason: entry.reason || ""
          });
        });
        self.probedAt = Date.now();
        self.publish();
        return self.nodes;
      }).catch(function (err) {
        self.probing = false;
        if (manual) toast("测速失败：" + String(err), "warn");
        return self.nodes;
      });
    },

    /**
     * The node `自动` picks: the lowest measured latency among the nodes a tunnel can
     * actually be built on. A node that only answers ICMP is reachable but unusable,
     * so it is not a candidate — see the table in the README.
     */
    best: function () {
      var best = null;
      this.nodes.forEach(function (node) {
        if (typeof node.latency_ms !== "number") return;
        var usable = node.probe_state === "ok" || node.probe_state === "slow" ||
          node.probe_state === null || node.probe_state === undefined;
        if (!usable) return;
        if (!best || node.latency_ms < best.latency_ms) best = node;
      });
      return best;
    }
  };

  /* ----------------------------------------------------------------- kernel */

  /*
   * The kernel child process: what it is doing, and what it is saying.
   *
   * `/api/kernel/log?since=N` answers with the lines the page has not seen *and* the
   * kernel's current state, so one poll keeps both the tunnel card and the log drawer
   * current. Polling runs only while a kernel is alive — an idle client asks for
   * nothing — and the state is read once at startup so the page can say whether a
   * kernel is installed at all.
   *
   * The endpoint never comes from here: it is read off the kernel's own stdout by the
   * shell, per the integration contract at hongshi.site/api.html.
   */
  var kernelStore = {
    info: null,
    seq: 0,
    timer: null,
    interval: 0,
    watching: false,
    /// The `/api/kernel/log` request currently in flight, if any. Two drains at
    /// once would ask for the same `since` and write the same lines twice.
    pending: null,
    subscribers: [],

    subscribe: function (listener) {
      var self = this;
      this.subscribers.push(listener);
      return function () {
        var index = self.subscribers.indexOf(listener);
        if (index >= 0) self.subscribers.splice(index, 1);
      };
    },

    publish: function () {
      var listeners = this.subscribers.slice();
      for (var i = 0; i < listeners.length; i++) {
        try {
          listeners[i](this.info);
        } catch (err) {
          /* one listener failing must not stop the others */
        }
      }
    },

    /** Read the state once, without starting the poll. */
    refresh: function () {
      var self = this;
      return apiJson("/api/kernel").then(function (result) {
        var body = (result && result.body) || {};
        if (body.kernel) {
          self.info = body.kernel;
          self.publish();
        }
        return self.info;
      }).catch(function () { return self.info; });
    },

    /** Append whatever the kernel has printed since the last look. */
    drain: function () {
      var self = this;

      // A tunnel starting calls `watch(true)` and `drain()` together, and both used
      // to fire a request with the same `since` — so the same server lines came back
      // twice and the panel showed "启动内核 … ×2", claiming two kernels for one.
      // The request already in flight is the answer the second caller wanted, and
      // nothing is skipped by sharing it: `since` only moves when a response lands.
      if (this.pending) return this.pending;

      var settled = function (value) {
        self.pending = null;
        return value;
      };
      this.pending = apiJson("/api/kernel/log?since=" + this.seq).then(function (result) {
        var body = (result && result.body) || {};
        if (typeof body.seq === "number") self.seq = body.seq;
        if (Array.isArray(body.lines)) {
          body.lines.forEach(function (line) { termWrite(line.text, "kernel"); });
        }

        var next = body.kernel;
        var changed = next && (!self.info ||
          self.info.state !== next.state ||
          self.info.running !== next.running ||
          self.info.endpoint !== next.endpoint ||
          self.info.found !== next.found);

        if (next) {
          self.info = next;
          tunnelStore.followKernel(next);
        }
        if (changed) {
          self.publish();
          // A kernel that started or stopped changes which rate is right.
          self.rearm();
        }
        return self.info;
      }).catch(function () { return self.info; }).then(settled, settled);
      return this.pending;
    },

    /** Poll while there is something to watch, faster while a kernel is alive. */
    watch: function (on) {
      this.watching = !!on;
      this.rearm();
      if (on) this.drain();
    },

    /**
     * Pick the poll rate, or stop polling entirely.
     *
     * Three states rather than two: a live kernel is worth a second's latency; an open
     * panel with nothing running only needs to notice a process that starts, and once
     * a second would be a timer ticking forever at an empty log; and with the panel
     * shut and nothing running the client asks for nothing at all.
     */
    rearm: function () {
      var want = 0;
      if (this.watching) {
        want = (this.info && this.info.running) ? KERNEL_POLL_MS : KERNEL_IDLE_POLL_MS;
      }
      if (want === this.interval) return;

      if (this.timer) window.clearInterval(this.timer);
      this.timer = null;
      this.interval = want;
      if (!want) return;

      var self = this;
      this.timer = window.setInterval(function () { self.drain(); }, want);
    },

    /** Ask the client to fetch this platform's kernel and install it. */
    download: function () {
      var self = this;
      return apiSend("/api/kernel/download", {}).then(function (result) {
        var body = (result && result.body) || {};
        if (body.kernel) {
          self.info = body.kernel;
          self.publish();
        }
        return { ok: !!(result.ok && body.state === "ok"), body: body };
      });
    }
  };

  /* ----------------------------------------------------------------- tunnel */

  /*
   * One tunnel, one place that owns it.
   *
   * The 联机 page starts it and the 主页 page reports it, so neither can keep the
   * state itself: whichever page is not on screen still has to be right when the
   * user navigates back to it. `tunnelStore` holds the facts, subscribers redraw
   * their own section, and any page that starts or stops a tunnel goes through
   * here.
   *
   * `createdAt` is a local timestamp rather than something the shell returns: the
   * interface wants "创建于 2 分钟前" ticking every second, and the shell's own
   * uptime counter is not the tunnel's.
   */
  var tunnelStore = {
    tunnel: null,
    createdAt: null,
    subscribers: [],

    running: function () {
      return !!this.tunnel;
    },

    /** Seconds since the tunnel was created, or null when none is running. */
    age: function () {
      if (!this.tunnel || !this.createdAt) return null;
      return Math.max(0, Math.floor((Date.now() - this.createdAt) / 1000));
    },

    set: function (tunnel) {
      this.tunnel = tunnel || null;
      this.createdAt = tunnel ? Date.now() : null;
      this.publish();
    },

    clear: function () {
      this.tunnel = null;
      this.createdAt = null;
      this.publish();
    },

    /**
     * Track the kernel child process.
     *
     * Called on every kernel poll. `createdAt` is deliberately *not* reset while the
     * tunnel stays up: the endpoint only exists once the kernel has printed it, and
     * re-stamping the creation time on each poll would make "已运行" restart every
     * second. A process that has exited clears the tunnel, because a room ends when
     * the kernel ends and there is no resume.
     */
    followKernel: function (info) {
      if (!info) return;

      if (!info.running) {
        if (this.tunnel) this.clear();
        return;
      }

      var current = this.tunnel;
      var next = {
        endpoint: info.endpoint || null,
        uuid: info.uuid || null,
        node: info.relay || null,
        relay: info.relay || null,
        game_port: info.game_port || null,
        mode: info.mode || (current ? current.mode : null)
      };
      var unchanged = current &&
        current.endpoint === next.endpoint &&
        current.uuid === next.uuid &&
        current.node === next.node;
      if (unchanged) return;

      this.tunnel = next;
      if (!this.createdAt) this.createdAt = Date.now();
      this.publish();
    },

    subscribe: function (listener) {
      var self = this;
      this.subscribers.push(listener);
      return function () {
        var index = self.subscribers.indexOf(listener);
        if (index >= 0) self.subscribers.splice(index, 1);
      };
    },

    publish: function () {
      // Copied before iterating: a listener is free to unsubscribe itself.
      var listeners = this.subscribers.slice();
      for (var i = 0; i < listeners.length; i++) {
        try {
          listeners[i](this.tunnel);
        } catch (err) {
          /* one section failing must not stop the others from redrawing */
        }
      }
    },

    /** Ask the shell for the current tunnel, so a reload shows the truth. */
    refresh: function () {
      var self = this;
      return apiJson("/api/tunnel/status").then(function (result) {
        var body = result.body || {};
        if (body.running && body.tunnel) self.set(body.tunnel);
        else if (!body.running) self.clear();
        return body;
      }).catch(function () {
        return {};
      });
    },

    /** Stop the tunnel, wherever the user did it from. */
    stop: function () {
      var self = this;
      return apiSend("/api/tunnel/stop", {}).then(function (result) {
        self.clear();
        termWrite("隧道已关闭", "dim");
        return result;
      });
    }
  };

  /* ------------------------------------------------------------------ toast */

  var toastEl = document.getElementById("toast");
  var toastTimer = null;

  function toast(message, tone, ms) {
    if (!toastEl) return;
    toastEl.textContent = message;
    toastEl.hidden = false;
    if (tone) toastEl.dataset.tone = tone; else delete toastEl.dataset.tone;
    window.clearTimeout(toastTimer);
    toastTimer = window.setTimeout(function () { toastEl.hidden = true; }, ms || 4200);
  }

  /* -------------------------------------------------------------- settings */

  var settings = null;
  /* Read-only facts the shell reports next to the settings: where the settings file
     is, and which HTTP stack this build talks to the site with. Never written back. */
  var settingsMeta = {};

  function adoptSettings(body) {
    if (!body) return;
    // `/api/settings` answers `{settings: {...}, path: "...", http_backend: "..."}`.
    // `settings` has to be the *inner* object, because every caller reads
    // `S.settings.api_base`; the two envelope fields are kept aside for the one page
    // that displays them. Storing the envelope itself — which is what this did —
    // left every field undefined, so the service address sat on "读取中…" forever and
    // the settings form opened blank.
    if (body.settings) {
      settings = body.settings;
      settingsMeta = { path: body.path, http_backend: body.http_backend };
    } else {
      settings = body;
    }
  }

  function loadSettings(force) {
    if (settings && !force) return Promise.resolve(settings);
    return apiJson("/api/settings").then(function (result) {
      if (result.ok && result.body) adoptSettings(result.body);
      return settings;
    });
  }

  function saveSettings(patch) {
    return apiSend("/api/settings", patch).then(function (result) {
      if (result.ok && result.body) adoptSettings(result.body);
      return result;
    });
  }

  /* ------------------------------------------------------------- formatting */

  function relativeTime(seconds) {
    if (typeof seconds !== "number" || !isFinite(seconds)) return "—";
    var total = Math.max(0, Math.floor(seconds));
    if (total < 5) return "刚刚";
    if (total < 60) return total + " 秒前";
    var minutes = Math.floor(total / 60);
    if (minutes < 60) return minutes + " 分钟前";
    var hours = Math.floor(minutes / 60);
    if (hours < 24) return hours + " 小时前";
    return Math.floor(hours / 24) + " 天前";
  }

  function duration(seconds) {
    if (typeof seconds !== "number" || !isFinite(seconds) || seconds < 0) return "—";
    var total = Math.floor(seconds);
    var hours = Math.floor(total / 3600);
    var minutes = Math.floor((total % 3600) / 60);
    var secs = total % 60;
    if (hours > 0) return hours + " 小时 " + minutes + " 分";
    if (minutes > 0) return minutes + " 分 " + secs + " 秒";
    return secs + " 秒";
  }

  /** The four latency states the node list paints. */
  function latencyState(ms) {
    if (typeof ms !== "number" || !isFinite(ms) || ms < 0) return "unknown";
    if (ms < 80) return "low";
    if (ms < 200) return "mid";
    return "high";
  }

  /*
   * Latency bands, for a node whose state is just "how fast".
   */
  var LATENCY_LABEL = {
    low: "良好",
    mid: "一般",
    high: "较慢",
    dead: "不可达",
    unknown: "未探测"
  };

  /*
   * Node states, which are not the same thing as latency bands.
   *
   * `ping` is the one worth naming: the host answered ICMP but its control port did
   * not, so it is reachable and unusable at the same time. Showing that as
   * "不可达" would blame the user's network, and showing it green would promise a
   * tunnel that cannot be created.
   *
   * `low`/`mid`/`high` are here too so one lookup covers both vocabularies — the
   * probe reports `ok`/`slow`/`ping`/`dead`/`none`, while a purely local guess uses
   * the latency bands.
   */
  var NODE_STATE = {
    ok: "可建隧道",
    slow: "可建隧道 · 较慢",
    ping: "仅能 ping 通",
    dead: "不可达",
    none: "不可达",
    unknown: "未探测",
    low: "可建隧道",
    mid: "可建隧道 · 一般",
    high: "可建隧道 · 较慢"
  };

  function latencyText(ms) {
    if (typeof ms !== "number" || !isFinite(ms)) return "—";
    return Math.round(ms) + " ms";
  }

  function escapeHtml(value) {
    return String(value == null ? "" : value)
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;");
  }

  /* Build an asset URL the browser has to resolve itself — a `<use href>`, an
     `<img src>`. It is just the path now: there is no session token to append, and
     the fragment splitting this used to do existed only so the token could go
     *before* the `#`. */
  function assetUrl(path) {
    return path;
  }

  function icon(name, extraClass) {
    return '<svg class="icon' + (extraClass ? " " + extraClass : "") +
      '" aria-hidden="true"><use href="' + escapeHtml(assetUrl("icons.svg#i-" + name)) +
      '"></use></svg>';
  }

  function copyText(text, what) {
    function done() { toast("已复制" + (what ? what : "") + "：" + text, null, 3200); }
    function failed() { toast("复制失败，请手动选择：" + text, "warn"); }

    if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(text).then(done, failed);
      return;
    }
    // Older browsers and non-secure contexts.
    try {
      var area = document.createElement("textarea");
      area.value = text;
      area.setAttribute("readonly", "");
      area.style.position = "fixed";
      area.style.opacity = "0";
      document.body.appendChild(area);
      area.select();
      var ok = document.execCommand("copy");
      document.body.removeChild(area);
      if (ok) done(); else failed();
    } catch (err) {
      failed();
    }
  }

  /* ------------------------------------------------------------------ theme */

  /* The four colors used for the tunnel summary, matching the design system. */
  var TONES = { ok: "ok", warn: "warn", err: "err", info: "info" };

  /* ------------------------------------------------------------- reporting */

  var health = null;

  function setLinkState(state, text) {
    var pill = document.getElementById("link-state");
    var label = document.getElementById("link-text");
    if (pill) {
      pill.dataset.state = state;
      // Collapsed, the pill is only a dot, so the words move to the tooltip
      // rather than disappearing.
      pill.title = text;
    }
    if (label) label.textContent = text;
  }

  function checkHealth(manual) {
    var started = Date.now();
    apiJson("/api/health").then(function (result) {
      if (!result.ok || !result.body) throw new Error("HTTP " + result.status);
      health = result.body;
      setLinkState("ok", "客户端就绪");
      if (manual) toast("已连接，往返 " + (Date.now() - started) + " ms");
      if (current && current.onHealth) current.onHealth(health);
    }).catch(function () {
      setLinkState("down", "已断开");
      if (manual) toast("客户端没有响应，可能已经退出了", "warn");
    });
  }

  /* ----------------------------------------------------------------- router */

  var routes = {};
  var current = null;
  var currentName = null;
  var booted = false;
  var pageEl = document.getElementById("page");
  var mainEl = document.getElementById("main");
  var NO_RENDER = { render: null };

  function register(name, handler) {
    routes[name] = handler;
  }

  function pathName() {
    var path = window.location.pathname.replace(/\/+$/, "");
    if (path === "" || path === "/index.html") return "home";
    return path.replace(/^\//, "");
  }

  /* Run a page builder so that a mistake inside one page cannot take the whole
     client down with it.

     It used to be called bare (`current = handler(pageEl) || NO_RENDER`), and that
     is not a theoretical hazard: a page that threw part-way through left the shell
     half-started — the router's caller was `boot()`, so the health poll, the
     one-second tick and the drawer wiring after it never ran at all, and the
     symptom was a status pill stuck on "连接中…" with no error anywhere on screen.
     A thrown page now costs that page and nothing else. */
  function renderPage(name, handler) {
    try {
      return handler(pageEl) || NO_RENDER;
    } catch (err) {
      if (window.console && window.console.error) {
        window.console.error("page " + name + " failed to render:", err);
      }
      var message = (err && err.message) ? err.message : String(err);
      pageEl.innerHTML =
        '<div class="notice notice--warn">' + icon("warn") +
        "<div><strong>这个页面没能画出来。</strong><br>" +
        "客户端本身还在运行，可以切到别的页面。错误：<code>" + escapeHtml(message) + "</code></div></div>";
      return {
        title: "出错了",
        detail: message,
      };
    }
  }

  function go(name, replace) {
    var target = name === "home" ? "/" : "/" + name;
    if (replace) window.history.replaceState(null, "", target);
    else window.history.pushState(null, "", target);
    show();
  }

  function markNav(name) {
    var items = document.querySelectorAll(".nav-item");
    for (var i = 0; i < items.length; i++) {
      var item = items[i];
      if (item.dataset.page === name) item.setAttribute("aria-current", "page");
      else item.removeAttribute("aria-current");
    }
  }

  function show() {
    var name = pathName();
    var handler = routes[name];

    if (!handler) {
      // The three services that are not built yet still have a page: it says so,
      // rather than pretending the link was wrong. Its return value has to be passed
      // on — discarding it (as this did) meant the page object was lost, so the tab
      // title stayed bare and the page got none of its own lifecycle hooks.
      handler = routes.soon ? function (el) { return routes.soon(el, name); } : null;
    }
    if (!handler) {
      go("home", true);
      return;
    }

    if (current && current.destroy) {
      try { current.destroy(); } catch (err) { /* a page must not break navigation */ }
    }

    markNav(name);
    pageEl.textContent = "";
    currentName = name;
    current = renderPage(name, handler);

    /*
     * Replay the arrival animation.
     *
     * `.page` is the same element for the life of the tab, so an animation declared
     * on it runs exactly once — on page load. Every navigation after that was a hard
     * cut, which is why the interface looked like it had no transition at all.
     * Removing the class, reading a layout property to force the style to be
     * recomputed, and adding it back is what restarts it. Done after the content is
     * in place so the new board is what blurs into focus.
     */
    pageEl.classList.remove("page--enter");
    void pageEl.offsetWidth;
    pageEl.classList.add("page--enter");

    document.title = (current.title ? current.title + " · " : "") + "红石联机";
    if (mainEl) mainEl.scrollTop = 0;
    window.scrollTo(0, 0);
  }

  function startRouter() {
    // Sidebar links are real hrefs, so they work before this script runs and are
    // shareable; intercepting the click is what makes navigation instant.
    document.addEventListener("click", function (event) {
      var link = event.target.closest ? event.target.closest("a[href^='/']") : null;
      if (!link) return;
      if (link.target || link.hasAttribute("download")) return;
      if (event.metaKey || event.ctrlKey || event.shiftKey || event.button !== 0) return;
      var name = link.getAttribute("href").replace(/^\//, "") || "home";
      if (!routes[name] && !routes.soon) return;
      event.preventDefault();
      go(name);
    });

    window.addEventListener("popstate", show);
    show();
  }

  /* ---------------------------------------------------------------- sidebar */

  function sideSet(collapsed) {
    if (!appEl) return;
    appEl.dataset.side = collapsed ? "collapsed" : "expanded";
    var toggle = document.getElementById("side-toggle");
    if (toggle) {
      toggle.setAttribute("aria-expanded", collapsed ? "false" : "true");
      toggle.setAttribute("aria-label", collapsed ? "展开侧栏" : "收起侧栏");
    }
    try { window.localStorage.setItem(SIDE_KEY, collapsed ? "1" : "0"); } catch (err) { /* ignore */ }
  }

  function sideStart() {
    var stored = null;
    try { stored = window.localStorage.getItem(SIDE_KEY); } catch (err) { /* ignore */ }
    // Narrow windows get the rail without being asked; the user's own choice wins
    // on a wide one.
    var narrow = window.matchMedia("(max-width: 860px)").matches;
    sideSet(stored === null ? narrow : stored === "1");

    var toggle = document.getElementById("side-toggle");
    if (toggle) {
      toggle.addEventListener("click", function () {
        sideSet(appEl.dataset.side !== "collapsed");
      });
    }
    var quit = document.getElementById("action-quit");
    if (quit) {
      quit.addEventListener("click", function () {
        quit.disabled = true;
        api("/api/shutdown", { method: "POST" }).then(function () {
          setLinkState("down", "已停止");
          toast("客户端正在退出，控制台窗口也会关闭", null, 6000);
        }).catch(function () {
          quit.disabled = false;
          toast("客户端没有响应退出请求，请在其控制台窗口按 Ctrl+C", "warn");
        });
      });
    }
  }

  /* ------------------------------------------------------------------ boot */

  function boot() {
    if (booted) return;
    booted = true;

    sideStart();
    startRouter();

    var clear = document.getElementById("drawer-clear");
    if (clear) clear.addEventListener("click", termClear);

    // The log panel's switch, in the top-right corner of the board. It is the panel's
    // only close control — the header used to carry a second one a few pixels away,
    // which read as one control that was broken rather than two that worked.
    var logToggle = document.getElementById("log-toggle");
    if (logToggle) {
      logToggle.addEventListener("click", function () { drawerOpen(!drawerIsOpen()); });
      logToggle.setAttribute("aria-expanded", drawerIsOpen() ? "true" : "false");
    }

    checkHealth(false);
    window.setInterval(function () { checkHealth(false); }, PAGE_POLL_MS);
    loadSettings(false);
    tunnelStore.refresh();

    // Whether a kernel is installed is a fact every page needs as soon as it draws, so
    // it is read at startup. The poll only starts once something is actually running.
    kernelStore.refresh().then(function (info) {
      if (info && info.running) kernelStore.watch(true);
    });

    // The relay list is read once here, at startup, and every visit to 联机 renders
    // from it. Probing is deliberately part of the same pass: the numbers are what
    // the page is for, and doing it now means the list is already measured by the
    // time the user gets there. A failure is not surfaced — the page shows the
    // reason and offers the button.
    nodeStore.load(false).then(function (nodes) {
      if (nodes.length) nodeStore.probe(false);
    });

    document.addEventListener("visibilitychange", function () {
      if (!document.hidden) {
        checkHealth(false);
        if (current && current.onTick) current.onTick();
      }
    });

    /*
     * The "创建于 …" counters.
     *
     * This ran every second, which is a wake-up, a DOM write and a `Date.now()` per
     * second for a number nobody reads to the second — and on a laptop it is a timer
     * that never lets the page go quiet. The counters are formatted coarsely anyway
     * ("2 分钟前", "1 小时 4 分"), so a slower tick shows the same thing a moment
     * later. Navigating refreshes them immediately, because `show()` renders the page.
     */
    window.setInterval(function () {
      if (current && current.onTick) current.onTick();
    }, TICK_MS);
  }

  /* ------------------------------------------------------------------ export */

  window.Shell = {
    // lifecycle: `pages.js` calls this once it has registered its routes. Both are
    // classic scripts, so `app.js` runs first and cannot assume a page exists yet.
    ready: boot,
    // networking
    api: api,
    apiJson: apiJson,
    apiSend: apiSend,
    // settings
    loadSettings: loadSettings,
    saveSettings: saveSettings,
    get settings() { return settings; },
    get settingsMeta() { return settingsMeta; },
    // log
    termWrite: termWrite,
    termClear: termClear,
    drawerOpen: drawerOpen,
    drawerIsOpen: drawerIsOpen,
    // router
    register: register,
    go: go,
    pathName: pathName,
    // the one tunnel and the one relay list every page reads from
    tunnel: tunnelStore,
    nodes: nodeStore,
    kernel: kernelStore,
    // ui helpers
    toast: toast,
    icon: icon,
    escapeHtml: escapeHtml,
    copyText: copyText,
    relativeTime: relativeTime,
    duration: duration,
    latencyState: latencyState,
    latencyText: latencyText,
    LATENCY_LABEL: LATENCY_LABEL,
    NODE_STATE: NODE_STATE,
    TONES: TONES,
    setLinkState: setLinkState,
    get health() { return health; }
  };

  // If `pages.js` never arrives — a 404, a caching accident, a syntax error — the
  // shell still starts and says so, rather than showing an empty board forever.
  document.addEventListener("DOMContentLoaded", function () {
    window.setTimeout(function () {
      if (!booted) {
        booted = true;
        setLinkState("down", "页面脚本未加载");
        if (pageEl) {
          pageEl.innerHTML =
            '<div class="notice notice--warn">' + icon("warn") +
            "<div><strong>页面脚本没有加载。</strong><br>" +
            "<code>pages.js</code> 没有被执行，或者执行时报错了。" +
            "可以打开浏览器控制台看具体错误，或在设置里确认 <code>--web-dir</code> 指向的目录是完整的。</div></div>";
        }
      }
    }, 2000);
  });
})();
