/* Cfharness Desktop — initialization script for HUB webviews (PLAN §3.4, §5.2–5.3; contract C3/C4).
 *
 * Injected by the app (src-tauri/src/bridge.rs `init_script`) into the webview of a registered hub only; the app bakes
 * in the hub origin and the badge poll interval. Rules:
 * - runs only in the top frame of that exact origin;
 * - reads NO DOM: the only page data it touches is the JSON of GET /app/badge (same origin, the hub's own cookie);
 * - carries no token/credential; sends only ints/bools/version to Rust (`badge_update`), speech commands (C4),
 *   clipboard text (≤ 1 MB) and Save-dialog downloads. Rust re-validates every payload and the caller origin.
 */
(function () {
  "use strict";
  var ORIGIN = __CFH_ORIGIN__;
  var POLL_S = __CFH_POLL_S__;
  try {
    if (window.top !== window || location.origin !== ORIGIN) return;
  } catch (_) {
    return;
  }
  if (window.cfhNative) return;

  var internals = null;
  function invoke(cmd, args) {
    if (!internals) {
      var t = window.__TAURI_INTERNALS__;
      if (!t || typeof t.invoke !== "function") return Promise.reject({ code: "unavailable" });
      internals = t;
    }
    return internals.invoke(cmd, args || {});
  }
  function codeOf(err) {
    var s = typeof err === "string" ? err : err && typeof err.code === "string" ? err.code : "";
    return /^[a-z][a-z-]{0,39}$/.test(s) ? s : "start";
  }

  // ---------------------------------------------------------------- badge (C3)
  var timer = null;
  function schedule(s) {
    var n = Math.max(5, Math.min(300, Number(s) || POLL_S));
    clearTimeout(timer);
    timer = setTimeout(poll, n * 1000);
  }
  function int(v) {
    return typeof v === "number" && isFinite(v) && Math.floor(v) === v && v >= 0 ? v : 0;
  }
  function poll() {
    fetch("/app/badge", { credentials: "same-origin", cache: "no-store", headers: { Accept: "application/json" } })
      .then(function (r) {
        if (r.status === 404) return "stop"; // hub without C3 (older version)
        if (!r.ok) return null; // 401 = not logged in yet, 503 = daemon down
        return r.json();
      })
      .then(function (j) {
        if (j === "stop") return;
        if (!j || typeof j !== "object") return schedule(POLL_S);
        var badge = {
          approvals: int(j.approvals),
          questions: int(j.questions),
          running: int(j.running),
          estop: j.estop === true,
          version: typeof j.version === "string" ? j.version.slice(0, 32) : "",
        };
        return invoke("badge_update", { badge: badge }).then(
          function (ack) { schedule(ack && ack.next_s); },
          function () { schedule(POLL_S); }
        );
      })
      .catch(function () { schedule(POLL_S); });
  }
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", function () { setTimeout(poll, 500); });
  else setTimeout(poll, 500);

  // ---------------------------------------------------------------- speech bridge (C4)
  var handlers = { result: [], end: [], error: [] };
  function dispatch(evt) {
    if (!evt || typeof evt !== "object" || !handlers[evt.kind]) return;
    var data;
    if (evt.kind === "result") data = { text: typeof evt.text === "string" ? evt.text : "", final: evt.final === true };
    else if (evt.kind === "error") data = { code: typeof evt.code === "string" ? evt.code : "native" };
    else data = {};
    handlers[evt.kind].slice().forEach(function (cb) {
      try { cb(data); } catch (_) {}
    });
  }
  Object.defineProperty(window, "__cfhNativeDispatch", { value: dispatch, writable: false, configurable: false, enumerable: false });

  var speech = Object.freeze({
    available: function () {
      return invoke("speech_available").then(function (v) { return v === true; }, function () { return false; });
    },
    start: function (opts) {
      var o = opts && typeof opts === "object" ? opts : {};
      var arg = { lang: o.lang === "en-US" ? "en-US" : "vi-VN", interim: o.interim !== false };
      return invoke("speech_start", { opts: arg }).then(
        function () {},
        function (e) { throw { code: codeOf(e) }; }
      );
    },
    stop: function () {
      return invoke("speech_stop").then(function () {}, function () {});
    },
    on: function (event, cb) {
      if (handlers[event] && typeof cb === "function") handlers[event].push(cb);
    },
  });

  var clipboard = Object.freeze({
    writeText: function (text) {
      return invoke("clipboard_write", { text: String(text) }).then(function () {});
    },
  });
  var download = Object.freeze({
    // data: base64 string; resolves true when the user saved the file
    save: function (name, data) {
      return invoke("download_save", { file: { name: String(name || ""), data: String(data || "") } });
    },
  });
  Object.defineProperty(window, "cfhNative", {
    value: Object.freeze({ speech: speech, clipboard: clipboard, download: download, app: "CfharnessDesktop" }),
    writable: false,
    configurable: false,
    enumerable: false,
  });

  // Copy buttons outside a secure context (G5): give the page a clipboard.writeText that goes through the app.
  try {
    if (!navigator.clipboard || typeof navigator.clipboard.writeText !== "function") {
      Object.defineProperty(navigator, "clipboard", { value: clipboard, configurable: true });
    }
  } catch (_) {}
})();
