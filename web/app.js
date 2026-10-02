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

  var PAGE_POLL_MS = 3000;
  /* How long the startup update check waits before it may put a modal on screen. Long
     enough that the first page has painted and settled, short enough that a user who
     opened the client to do one thing still sees it. */
  var UPDATE_CHECK_DELAY_MS = 2500;
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

    // The panel has no switch in the top bar any more, so the page that opens it is
    // also the only thing that says it is open — `aria-expanded` lives on whatever
    // control called this, not here. What this does keep is the one thing the panel
    // itself cannot: Escape. Without it the only ways out are navigating away or
    // reloading, because the header deliberately has no close button.
    drawer.setAttribute("aria-hidden", open ? "false" : "true");

    // The kernel's log is only worth polling while something is worth watching: a
    // live kernel, or an open panel waiting for one.
    if (open) kernelStore.watch(true);
    else kernelStore.watch(!!(kernelStore.info && kernelStore.info.running));
  }

  /* Escape closes the log panel. On `document`, not on the drawer: the drawer is not
     focused when it opens, so a listener on it would never fire, and the key has to
     work from wherever the user's focus happens to be. */
  document.addEventListener("keydown", function (event) {
    if (event.key !== "Escape" && event.key !== "Esc") return;
    if (!drawerIsOpen()) return;
    drawerOpen(false);
  });

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

  /* ----------------------------------------------------------------- dialogs */

  /*
   * One dialog at a time, and here rather than in a page, because the client has three of
   * them and only one of the three belongs to a page: the room address (联机), the
   * new-version notice and the first-run guide (both the shell's own). Writing the same
   * overlay in three places is how a client ends up with three corner radii.
   *
   * Everything the shell opens shares the same three ways out — the button, the backdrop
   * and Escape — and that is what stops a dialog from feeling like a trap. They differ
   * only in what they say, so the API is markup in, element out.
   */
  var openOverlay = null;
  var openDialogElement = null;
  var openOnClose = null;

  /** Close whatever is open. Safe to call when nothing is. */
  function closeDialog() {
    var overlay = openOverlay;
    var onClose = openOnClose;
    try {
      document.removeEventListener("keydown", onDialogEscape);
      document.documentElement.classList.remove("has-dialog");
    } finally {
      openOverlay = null;
      openDialogElement = null;
      openOnClose = null;
      if (overlay) overlay.remove();
    }
    // After the overlay is gone, so a dialog this opens is the only one on screen. It runs
    // for every way out — the button, Escape, the backdrop — because the callers that want
    // it are asking "was this message seen", and which key dismissed it does not matter.
    if (onClose) onClose();
  }

  function onDialogEscape(event) {
    if (event.key === "Escape" || event.key === "Esc") closeDialog();
  }

  /**
   * Show a dialog. `html` is the whole panel, and `after` gets the panel and the overlay
   * so the caller can wire its own controls.
   *
   * Only one is ever open: opening a second closes the first, which is not just tidiness
   * — the update notice fires on startup and the room dialog can be triggered a second
   * later, and two stacked overlays look like a bug in the page rather than like two
   * messages.
   *
   * `onClose` runs when this dialog goes away, however it goes away. It exists for the one
   * caller that has something to say *after* a message rather than instead of it — the
   * tenth-launch thank-you, which waits for the room dialog to be dismissed.
   */
  function openDialog(html, after, wide, onClose) {
    closeDialog();

    var overlay = document.createElement("div");
    overlay.className = "dlg-overlay";
    overlay.innerHTML = '<div class="dlg' + (wide ? " dlg--wide" : "") +
      '" role="dialog" aria-modal="true">' + html + "</div>";

    document.body.appendChild(overlay);

    openOverlay = overlay;
    openDialogElement = overlay.firstElementChild;
    openOnClose = typeof onClose === "function" ? onClose : null;

    // A dialog can open *under* the pointer that asked for it: the room dialog appears
    // about 200ms after 开启房间 is pressed, so the second click of a double-click — or an
    // automated click that retried because the button it aimed at had just been covered —
    // lands on the backdrop and dismisses the message before it can be read. Ignoring
    // backdrop clicks for the first third of a second costs a deliberate dismissal nothing
    // and stops a dialog from being closed by the gesture that opened it.
    var openedAt = Date.now();
    overlay.addEventListener("click", function (event) {
      // The backdrop only. A click that started inside the panel and drifted out would
      // otherwise close the dialog the user was reading.
      if (event.target !== overlay) return;
      if (Date.now() - openedAt < 350) return;
      closeDialog();
    });
    document.addEventListener("keydown", onDialogEscape);

    var close = overlay.querySelector("[data-dialog-close]");
    if (close) close.addEventListener("click", closeDialog);

    if (typeof after === "function") after(openDialogElement, overlay);
    return openDialogElement;
  }

  function dialogIsOpen() {
    return !!openOverlay;
  }

  /**
   * The standard header and footer, so every dialog's title reads the same size and its
   * buttons land in the same corner.
   *
   * `actions` is markup rather than a list of descriptors: each dialog's buttons do
   * genuinely different things, and a descriptor format that could express all three
   * would be longer than the markup it replaced.
   */
  function dialogTitle(text) {
    return '<h2 class="dlg-title">' + escapeHtml(text) + "</h2>";
  }

  function dialogActions(actions) {
    return '<div class="dlg-actions">' + actions + "</div>";
  }

  function dialogCloseButton() {
    return '<button type="button" class="dlg-close" data-dialog-close aria-label="关闭">' +
      icon("close", "icon--sm") + "</button>";
  }

  /* ------------------------------------------------------------------ guide */

  /** Where "this user has seen the guide" is remembered. */
  var GUIDE_SEEN_KEY = "hongshi.shell.guide.seen";

  /** Air around a lifted control: how far the ring stands off it, in pixels. */
  var GUIDE_PAD = 8;

  function guideSeen() {
    try {
      return window.localStorage.getItem(GUIDE_SEEN_KEY) === "1";
    } catch (err) {
      // No storage (private mode): the guide then shows on every launch. The annoying
      // failure is the safe one here — the alternative is a first run with no guidance.
      return false;
    }
  }

  function rememberGuideSeen() {
    try {
      window.localStorage.setItem(GUIDE_SEEN_KEY, "1");
    } catch (err) {
      /* ignoring storage is the documented fallback above */
    }
  }

  /*
   * The first-run guide is a **spotlight**, not a slideshow.
   *
   * The first version was four dialogs with a diagram each, and it was the wrong shape for
   * the job: it described the interface in a place where the interface was not, so the user
   * read about a button and then had to find it. A spotlight puts the words next to the
   * thing they are about, on the real page, and gets out of the way one element at a time.
   *
   * Each step is therefore a *target plus a sentence*, and the guide never invents a
   * surface of its own: it dims everything and lifts one real control out of the dim.
   */
  var GUIDE_STEPS = [
    {
      target: '.nav-item[data-page="connect"]',
      where: "below",
      text: "点击切换联机页。",
      // Nothing to wait for: the top bar is in the static markup and is on every page.
      advanceOn: "click",
      // The internal router, not `Shell.go`: this file *is* `window.Shell`, so there is
      // no `S` to reach through here.
      prepare: function () { go("home"); },
    },
    {
      target: "#room-relay",
      /*
       * Above, not below, and this is the one placement in the guide that is not a
       * preference. The relay list opens downwards out of this control, so a card under it
       * does not merely look cluttered — it sits on top of the list the user was just asked
       * to read, and the rows underneath it cannot be clicked at all.
       */
      where: "above",
      text: "选择一台离你最近的服务器。不确定的话就选「自动选择」，它会挑延迟最低的那个。",
      /*
       * This step's action is deliberately *not* the way on.
       *
       * Clicking this control opens the list; it does not finish the choice. Advancing on
       * that first click would move the ring down to the port row while the list the user
       * was just told to read is still hanging open under it, and a tour that walks away
       * mid-sentence is worse than one that waits to be dismissed. So this step and the
       * next one carry a button; the first and last advance by doing the thing.
       */
      nextLabel: "知道了",
    },
    {
      target: ".room-port-row",
      where: "above",
      // The link is a real `href` rather than a binding, so the browser's own affordances —
      // middle click, copy link, open in a new tab — work, and it goes through the router
      // like every other internal link. It leaves the tour: walking off to read the help
      // is not the same as saying "I am done with this", so `finishGuide(false)` below
      // does not write the seen flag and the last step is still there next launch.
      text: "红石会自动帮你探测游戏端口（前提是游戏已经开了局域网）。你也可以自己填。" +
        ' <a class="guide-link" href="/help/port">不知道什么是游戏端口？点我</a>',
      /*
       * The other button, and this is the step that needs it for a plain reason: it
       * describes a *fact* (the port is detected) and has no action of its own, so without
       * a button the only way on is 跳过 — the opposite of what the step is for.
       */
      nextLabel: "知道了",
    },
    {
      target: "#room-start",
      // The top of the page, not above the button: see `where: "top"` in `placeGuide`.
      where: "top",
      text: "最后点这里就能开启房间了。开好之后把弹出的地址发给朋友，他在游戏里填上就能进来。",
      advanceOn: "click",
      /*
       * The fallback button, and the *conditional* sentence above it.
       *
       * Two things go wrong if this step is left to advance on its own click:
       *
       *   1. Without a kernel the button is `disabled`, and a disabled button dispatches no
       *      click event at all — so the guide could never be finished by doing the thing
       *      it asks for. The 知道了 button is the way out.
       *   2. A user who is told to press a button that will not press needs to be told why.
       *      `prepare` reads the kernel store and adds a line naming the download button
       *      when the kernel is missing, which is exactly the situation the guide exists to
       *      explain.
       */
      prepare: function () {
        var kernel = kernelStore.info;
        if (!kernel || kernel.found) return;
        GUIDE_STEPS[3].text =
          "最后点这里就能开启房间了。开好之后把弹出的地址发给朋友，他在游戏里填上就能进来。" +
          '<br><br>不过现在它还是灰的：<strong>你还没装内核</strong>。' +
          "先点上面那个「自动下载内核」，装好之后这个按钮就能按了。";
      },
      nextLabel: "知道了",
    },
  ];
  var guideState = null;

  /**
   * Run the guide.
   *
   * Returns whether it ran, so a caller can decide what to do instead — the point of a
   * guide is to be over, not to be a mode.
   */
  function showGuide() {
    if (guideSeen()) return false;
    if (guideState) return true;

    var overlay = document.createElement("div");
    overlay.className = "guide";
    overlay.innerHTML =
      '<div class="guide-ring" id="guide-ring"></div>' +
      '<div class="guide-card" id="guide-card">' +
      '<p class="guide-text" id="guide-text"></p>' +
      '<div class="guide-foot">' +
      '<span class="guide-count" id="guide-count"></span>' +
      '<button type="button" class="guide-next" id="guide-next" hidden></button>' +
      '<button type="button" class="guide-skip" id="guide-skip">跳过</button>' +
      "</div></div>";

    document.body.appendChild(overlay);

    guideState = {
      index: 0,
      overlay: overlay,
      ring: overlay.querySelector("#guide-ring"),
      card: overlay.querySelector("#guide-card"),
      text: overlay.querySelector("#guide-text"),
      count: overlay.querySelector("#guide-count"),
      timer: null,
      watching: null,
      cleanups: [],
    };

    /*
     * Clicking the dimmed area ends the guide, because a guide that will not go away is
     * worse than no guide.
     *
     * This is a listener on the *document* rather than on the overlay, and that is not a
     * detail: the overlay is `pointer-events: none` so the page underneath stays live, which
     * also means the overlay never receives a click to react to. The capture phase, so a
     * click the page stops before it bubbles is still noticed here.
     *
     * "Was this click meant for the lifted control?" is answered by *geometry*, and that is
     * not fussiness either. Every page here replaces its own contents on each store
     * publication; when a replacement lands between the press and the release, the browser
     * retargets the click at the common ancestor, so a press the user aimed squarely at the
     * relay picker can arrive with `#room-body` as its `target`. Coordinates are what the
     * user actually aimed at.
     */
    guideState.onDocumentClick = function (event) {
      if (!guideState) return;
      // A keyboard click (Enter on the focused control) reports 0,0. Tab-to-it-and-press is
      // not a click on the dimmed page, so it is never an exit.
      if (!event.clientX && !event.clientY) return;
      if (guideInside(event)) return;
      finishGuide(false);
    };
    document.addEventListener("click", guideState.onDocumentClick, true);

    // Escape is the same kind of "no thanks" as 跳过, and deliberate rather than incidental,
    // so unlike a stray click it is remembered.
    guideState.onKey = function (event) {
      if (event.key !== "Escape") return;
      event.preventDefault();
      finishGuide(true);
    };
    document.addEventListener("keydown", guideState.onKey, true);

    overlay.querySelector("#guide-skip").addEventListener("click", function (event) {
      event.stopPropagation();
      finishGuide(true);
    });

    // The link in step 3 leaves the tour to go and read something, so the ring stops
    // pointing at a row that is no longer on the page. Not remembered: walking off to read
    // the help is not the same as saying "I am done with this", and the last step has not
    // been seen yet.
    overlay.addEventListener("click", function (event) {
      if (event.target.closest(".guide-link")) finishGuide(false);
    });

    // The explicit "go on" control. Only the steps that describe a fact use it — see
    // `nextLabel` in `GUIDE_STEPS` — and it is one button whose label and visibility are
    // set per step rather than one control per step.
    overlay.querySelector("#guide-next").addEventListener("click", function (event) {
      event.stopPropagation();
      if (guideState) stepGuide(guideState.index + 1);
    });

    /*
     * A resize or a scroll invalidates every coordinate the guide is holding. Re-placing
     * is the honest response; leaving a ring where the button used to be is the kind of bug
     * that makes a guide look broken rather than merely stale.
     */
    guideState.onViewportChange = function () { placeGuide(); };
    window.addEventListener("resize", guideState.onViewportChange);
    window.addEventListener("scroll", guideState.onViewportChange, true);

    stepGuide(0);
    return true;
  }

  /**
   * Show one step, and wait for its target if the page is not there yet.
   *
   * The waiting is the part that makes this work at all: step 1 tells the user to click
   * 联机, so by step 2 the router is mid-navigation and the relay picker does not exist.
   * A guide that looked for its target once and gave up would fail on the one step whose
   * whole subject is "go to the other page".
   */
  function stepGuide(index) {
    var state = guideState;
    if (!state) return;

    state.index = index;
    clearGuideTimers();

    var step = GUIDE_STEPS[index];
    if (!step) {
      // Past the last step: the tour was walked to the end, which counts as seen.
      finishGuide(true);
      return;
    }

    if (step.prepare) step.prepare();

    state.count.textContent = "第 " + (index + 1) + " / " + GUIDE_STEPS.length + " 步";
    state.text.innerHTML = step.text;

    // One button, shown only by the steps that need it.
    var nextButton = state.overlay.querySelector("#guide-next");
    nextButton.hidden = !step.nextLabel;
    if (step.nextLabel) nextButton.textContent = step.nextLabel;

    state.watching = function () {
      if (!guideState) return;
      var target = document.querySelector(step.target);
      // A target with no box is a target that is not on screen yet — a page still
      // animating in, a list still rendering. Keep waiting rather than pointing at 0,0.
      if (!target || !target.getBoundingClientRect().width) {
        state.timer = window.setTimeout(state.watching, 90);
        return;
      }
      placeGuide();
      if (step.advanceOn === "click") watchForAdvance(step, target);
    };
    state.watching();
  }

  /**
   * Move on when the user does the thing the step asked for.
   *
   * Listening at the document in the capture phase, because the click may be stopped by
   * the page before it bubbles — the router calls `preventDefault` on the nav link, and a
   * listener that waited for the bubble would never hear the click it depends on.
   */
  function watchForAdvance(step, target) {
    var state = guideState;
    var handler = function (event) {
      if (!target.contains(event.target)) return;
      detach();
      // One frame, so a click that changes the page has swapped it before the next step
      // goes looking for its own target.
      window.requestAnimationFrame(function () {
        if (guideState) stepGuide(state.index + 1);
      });
    };
    var detach = function () {
      document.removeEventListener("click", handler, true);
      state.cleanups = state.cleanups.filter(function (entry) { return entry !== detach; });
    };

    document.addEventListener("click", handler, true);
    state.cleanups.push(detach);
  }

  /** Put the ring on the target and the card where it fits. */
  function placeGuide() {
    var state = guideState;
    if (!state) return;
    var step = GUIDE_STEPS[state.index];
    if (!step) return;

    var target = document.querySelector(step.target);
    if (!target || !target.getBoundingClientRect().width) return;

    var box = target.getBoundingClientRect();
    var pad = GUIDE_PAD;
    var vw = window.innerWidth;
    var vh = window.innerHeight;

    // The ring is drawn as a huge outline, which is what makes the dimming follow a
    // rounded box: the ring is transparent and its `box-shadow` does the shading, so the
    // shape of the hole is exactly the shape of the element.
    state.ring.style.top = (box.top - pad) + "px";
    state.ring.style.left = (box.left - pad) + "px";
    state.ring.style.width = (box.width + pad * 2) + "px";
    state.ring.style.height = (box.height + pad * 2) + "px";

    /*
     * Nothing is drawn over the target but a shadow, so there is no click-through layer to
     * manage: the lifted control is the one the user presses. The ring is a fixed box the
     * size of the target plus a little air, and its `box-shadow` dims everything else.
     */
    var card = state.card;
    card.style.maxWidth = Math.min(340, vw - 32) + "px";
    card.style.left = "0px";
    card.style.top = "0px";
    var cardBox = card.getBoundingClientRect();

    var margin = 12;
    var left = box.left + box.width / 2 - cardBox.width / 2;
    left = Math.max(margin, Math.min(left, vw - cardBox.width - margin));

    var below = box.bottom + pad + margin;
    var above = box.top - pad - margin - cardBox.height;
    /*
     * Prefer what the step asked for, then fall back to whichever side has the room, then to
     * the bottom of the window. A card pushed off screen is worse than one on the "wrong"
     * side of its target.
     *
     * `where: "top"` is the one placement that is not about the target at all: it means "the
     * top of the page, under the top bar", and it exists for the last step, whose target
     * sits near the bottom of a window with nothing below it. A card above that target is
     * the only other option, and it lands squarely on the two controls the step talks about
     * — the 开启房间 button itself and, with no kernel installed, the download button the
     * same sentence tells the user to press. Covering the page's heading instead is the
     * cheapest place to lose 150 pixels.
     */
    var topBar = parseFloat(
      window.getComputedStyle(document.documentElement).getPropertyValue("--topbar-h"));
    var top;
    if (step.where === "below" && below + cardBox.height <= vh - margin) top = below;
    else if (step.where === "above" && above >= margin) top = above;
    else if (step.where === "top") top = (topBar > 0 ? topBar : 60) + margin;
    else if (below + cardBox.height <= vh - margin) top = below;
    else if (above >= margin) top = above;
    else top = Math.max(margin, vh - cardBox.height - margin);

    card.style.left = Math.round(left) + "px";
    card.style.top = Math.round(top) + "px";
  }

  /**
   * Whether a click at these coordinates was aimed at the guide rather than past it.
   *
   * Three boxes count as inside: the card, the lifted control, and any list that control
   * has opened. The last one is the reason this is a list and not a single test — a popover
   * belongs to the thing that opened it and hangs outside its box, so choosing a relay is
   * part of the step rather than an exit from it.
   */
  function guideInside(event) {
    var state = guideState;
    if (!state) return false;

    var boxes = [state.card.getBoundingClientRect()];
    var target = document.querySelector(GUIDE_STEPS[state.index].target);
    if (target) boxes.push(target.getBoundingClientRect());
    var open = document.querySelector(".picker-list:not([hidden])");
    if (open) boxes.push(open.getBoundingClientRect());

    for (var i = 0; i < boxes.length; i++) {
      var box = boxes[i];
      // A little air, so the edge of the ring is still the ring.
      if (event.clientX >= box.left - GUIDE_PAD && event.clientX <= box.right + GUIDE_PAD &&
          event.clientY >= box.top - GUIDE_PAD && event.clientY <= box.bottom + GUIDE_PAD) {
        return true;
      }
    }
    return false;
  }

  function clearGuideTimers() {
    var state = guideState;
    if (!state) return;
    window.clearTimeout(state.timer);
    state.timer = null;
    state.cleanups.forEach(function (detach) { detach(); });
    state.cleanups = [];
  }

  /**
   * Take the guide off the screen. Safe to call when it is not running.
   *
   * `remember` says whether this was the user deciding they are done with the guide — 跳过,
   * Escape, or reaching the end — as opposed to a click that happened to land somewhere
   * else, or the step-3 link that walks off to read the help. Only the deliberate exits
   * write the flag, so a stray click does not cost somebody the rest of the tour, and a
   * real 跳过 is honoured for good.
   */
  function finishGuide(remember) {
    var state = guideState;
    if (!state) return;

    clearGuideTimers();
    guideState = null;

    window.removeEventListener("resize", state.onViewportChange);
    window.removeEventListener("scroll", state.onViewportChange, true);
    document.removeEventListener("click", state.onDocumentClick, true);
    document.removeEventListener("keydown", state.onKey, true);
    state.overlay.remove();
    if (remember) rememberGuideSeen();
  }

  /**
   * Show the guide on a first run, and only on a first run.
   *
   * The `seen` flag is the primary test, but it is not sufficient on its own: a build
   * handed to somebody who has been using the client for weeks would show them the guide
   * again, which reads as the program having forgotten them. A kernel already installed is
   * evidence of a previous run, so it counts as "seen" and is recorded as such — the flag
   * is written, not just checked, so the work is done once rather than on every launch.
   */
  function announceGuide() {
    if (guideSeen()) return;

    var kernel = kernelStore.info;
    if (kernel && kernel.found) {
      rememberGuideSeen();
      return;
    }
    showGuide();
  }

  /* ------------------------------------------------------------------ updates */


  /** Where "the user has already been told about this version" is remembered. */
  var UPDATE_SEEN_KEY = "hongshi.shell.update.seen";

  function updateSeen(version) {
    try {
      return window.localStorage.getItem(UPDATE_SEEN_KEY) === version;
    } catch (err) {
      // No storage (private mode): the notice then shows once per launch rather than
      // once per version. Annoying, never wrong.
      return false;
    }
  }

  function rememberUpdateSeen(version) {
    try {
      window.localStorage.setItem(UPDATE_SEEN_KEY, version);
    } catch (err) {
      /* ignoring storage is the documented fallback above */
    }
  }

  /**
   * Which build this browser would need, in the download endpoint's vocabulary.
   *
   * Read from the browser rather than from the client, and that is not a guess: the
   * *page* is what is being replaced, so the browser is the thing whose platform matters.
   */
  function platformQuery() {
    var ua = (navigator.userAgent || "").toLowerCase();
    var platform = (navigator.platform || "").toLowerCase();

    var os = "windows";
    if (ua.indexOf("mac") >= 0 || platform.indexOf("mac") >= 0) os = "macos";
    else if (ua.indexOf("linux") >= 0 || platform.indexOf("linux") >= 0) os = "linux";

    var arch = /arm64|aarch64/.test(ua) || /arm/.test(platform) ? "arm64" : "amd64";
    return "?kind=webui&platform=" + os + "&arch=" + arch;
  }

  /**
   * Hand the new build to the browser as an ordinary download.
   *
   * A normal download rather than a self-replacing update: the artifact the site serves
   * is a whole new executable, and an application that overwrites its own binary while it
   * is running is a much larger promise than this client wants to make. The same URL the
   * site's download page links to is the one used here, so there is one artifact and one
   * way to get it.
   */
  function openUpdateDownload(body) {
    // The version answer carries the `api_base` it was checked against, and that is
    // preferred over the cached settings: the two agree, but the body is *evidence* of
    // which site was asked, while the cache is whatever the last load left behind — and
    // this runs two and a half seconds after boot, when that load may still be in flight.
    var base = ((body && body.api_base) || (settings && settings.api_base) || "").replace(/\/+$/, "");
    if (!base) {
      toast("还不知道官方站点地址，先去设置页填上", "warn", 8000);
      return;
    }
    window.open(base + "/api/download/webui" + platformQuery(), "_blank", "noopener");
  }

  /**
   * Ask the official site what the latest version is.
   *
   * Resolves with a body that always has `current` and `channel`; `remote` and `update`
   * are `null` when the check could not be made, which is a third answer and not a
   * failure — the settings page shows all three, and the startup notice shows none of
   * them. See `version_endpoint` in http_server.rs.
   */
  function checkVersion() {
    return apiJson("/api/version").then(function (result) {
      return (result && result.body) || null;
    }).catch(function () {
      return null;
    });
  }

  /**
   * The startup notice: tell the user once per version, and never make them go looking.
   *
   * Called from `boot()` after the first render settles. It is deliberately quiet about
   * every outcome except one — a newer version exists and the user has not been told about
   * *this* version yet. "已是最新" is not news worth a modal, and a failed check is not the
   * user's problem at startup; the settings page is where all three answers belong.
   *
   * Once per version rather than once per launch, because a notice that returns every
   * morning is a notice people learn to dismiss without reading.
   */
  function announceUpdate() {
    checkVersion().then(function (body) {
      if (!body || body.update !== true || !body.remote) return;
      if (updateSeen(body.remote)) return;

      openDialog(
        dialogCloseButton() +
        dialogTitle("有新版本") +
        "<p>官方站点上是 <strong>v" + escapeHtml(body.remote) + "</strong>，你正在用的是 <strong>v" +
        escapeHtml(body.current) + "</strong>。</p>" +
        '<p class="dlg-note">下载后替换掉现在的客户端就行，设置和联机记录都留在原处。</p>' +
        dialogActions(
          '<button type="button" class="btn btn--ghost" data-dialog-close>以后再说</button>' +
          '<button type="button" class="btn btn--primary" id="update-download">' +
          icon("download", "icon--sm") + "去下载新版本</button>"
        ),
        function (panel) {
          var go = panel.querySelector("#update-download");
          go.addEventListener("click", function () {
            rememberUpdateSeen(body.remote);
            openUpdateDownload(body);
            closeDialog();
          });
          // Dismissing also counts as "told": the point of the notice is that the user
          // should not have to go looking, not that they must act.
          panel.querySelectorAll("[data-dialog-close]").forEach(function (button) {
            button.addEventListener("click", function () { rememberUpdateSeen(body.remote); });
          });
        }
      );
    });
  }

  /* ----------------------------------------------------------- sponsorship */

  /** Where the tip jar is. One place, because the dialog is not the only thing that may
      ever want to point at it. */
  var SUPPORT_URL = "https://ifdian.net/a/RedstoneOnline";

  /**
   * How often to say thank you: every tenth launch of the client.
   *
   * A round number the user can see for themselves ("第 10 次"), and one that cannot
   * arrive twice in a week of heavy play the way a time-based interval would. It counts
   * *launches*, not rooms: somebody who opens five rooms in one evening has played one
   * evening, and the counter comes from the client (`/api/health`) rather than from
   * `localStorage`, so it is not reset by clearing site data or doubled by a second
   * browser.
   */
  var SUPPORT_EVERY = 10;

  /** Once per run: opening a second room in the same launch must not ask again. */
  var supportOffered = false;

  /**
   * Say thank you on the tenth launch — but only to somebody who just played.
   *
   * Called when the *room* dialog goes away, and that timing is the whole design. It is
   * the one moment in the product where the user has demonstrably played with somebody:
   * the address exists, it has been copied, and a friend is on the other end of it. Asking
   * at startup would be asking a stranger, and asking on a launch where no room ever came
   * up would be thanking somebody for a session that did not happen.
   *
   * The cost of getting this wrong is not symmetric, which is why it is this quiet: a
   * missed tenth launch costs nothing, and an interruption that arrives while somebody is
   * waiting for an address costs the thing the dialog is asking for.
   */
  function announceSupport() {
    if (supportOffered) return false;

    var launches = health && health.launches;
    if (!launches) {
      // Normally the boot health poll has answered long before a room can be opened. If it
      // has not (a reload with a room already up), ask once rather than silently skipping
      // the launch this feature exists for.
      apiJson("/api/health").then(function (result) {
        if (result && result.ok && result.body) offerSupport(result.body.launches);
      });
      return false;
    }
    return offerSupport(launches);
  }

  /** The modulo check and the dialog. Split out so the late health answer can reuse it. */
  function offerSupport(launches) {
    if (supportOffered || !launches || launches % SUPPORT_EVERY !== 0) return false;
    supportOffered = true;

    openDialog(
      dialogCloseButton() +
      dialogTitle("第 " + launches + " 次一起玩") +
      "<p>这是你第 <strong>" + launches + "</strong> 次使用红石联机与朋友游玩啦。" +
      "如果你觉得红石联机好用的话，可以去爱发电赞助我们哦，谢谢你的支持！</p>" +
      // The address in full, not behind the button: a tip link nobody can read before
      // clicking is a link people do not click.
      '<p class="dlg-note">爱发电 · ' + escapeHtml(SUPPORT_URL) + "</p>" +
      dialogActions(
        '<button type="button" class="btn btn--ghost" data-dialog-close>下次一定</button>' +
        '<button type="button" class="btn btn--primary" id="support-go">' +
        icon("link", "icon--sm") + "去赞助</button>"
      ),
      function (panel) {
        panel.querySelector("#support-go").addEventListener("click", function () {
          // A tab rather than a navigation: this is a side quest, and the room the user
          // just opened has to stay on screen behind it.
          window.open(SUPPORT_URL, "_blank", "noopener");
          closeDialog();
        });
      }
    );
    return true;
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
     *before* the `#`.

     The leading slash is load-bearing rather than tidy. Routes can be more than one
     segment deep (`/help/port`), and a relative `icons.svg#i-play` on that page resolves
     against `/help/` — so every icon in the shell disappears at once, and an image in a
     step-by-step guide arrives broken. This helper is the one place that knows how a shell
     asset is addressed, which is why the slash belongs here and not at forty call sites. */
  function assetUrl(path) {
    return path.charAt(0) === "/" ? path : "/" + path;
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

  /* ----------------------------------------------------------------- chrome */

  /* The top bar itself is static markup: its destinations are real `href`s that the
     router intercepts, so there is nothing to wire except Quit.
     
     This used to be `sideStart` and it used to own a collapsible sidebar — the
     toggle, its `aria-expanded`/`aria-label` bookkeeping, and a `localStorage` key
     remembering whether the rail was collapsed. The navigation moved to the top bar
     and needs none of that: a four-item horizontal bar has no collapsed state to
     remember. `hongshi.shell.side` is left in whatever store it was written to
     rather than read; a stale preference for a control that no longer exists is not
     worth a migration step. */
  function wireChrome() {
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

    wireChrome();
    startRouter();

    var clear = document.getElementById("drawer-clear");
    if (clear) clear.addEventListener("click", termClear);

    // There is no log switch in the top bar to wire any more. The panel is opened
    // from the page that needs it (`Shell.drawerOpen(true)`, the 联机 page's 查看日志
    // and 服务 card both do this) and closed with Escape, which `drawerOpen` installs
    // a document-level listener for. The drawer's own `[hidden]` rule keeps it from
    // painting on load, so nothing here has to put it in its initial state.

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

    /*
     * The first run: the guide, or the update notice, and never both at once.
     *
     * They are queued rather than raced. The guide is about the product and the notice is
     * about the build, so the guide goes first — a user who has never opened the client is
     * not helped by being told there is a newer version of it. `openDialog` replaces
     * whatever is on screen, so firing both would silently drop the first.
     *
     * Both wait for the kernel store, because "has this person run the client before" is
     * answered by whether a kernel is installed, and that read is in flight at this point.
     */
    kernelStore.refresh().then(function () {
      window.setTimeout(function () {
        if (dialogIsOpen()) return;
        announceGuide();
        if (dialogIsOpen()) return;
        announceUpdate();
      }, UPDATE_CHECK_DELAY_MS);
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
    assetUrl: assetUrl,
    escapeHtml: escapeHtml,
    openDialog: openDialog,
    closeDialog: closeDialog,
    dialogIsOpen: dialogIsOpen,
    dialogTitle: dialogTitle,
    dialogActions: dialogActions,
    dialogCloseButton: dialogCloseButton,
    checkVersion: checkVersion,
    openUpdateDownload: openUpdateDownload,
    announceUpdate: announceUpdate,
    announceSupport: announceSupport,
    showGuide: showGuide,
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