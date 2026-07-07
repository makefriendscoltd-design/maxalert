// window.api 호환 래퍼 — Electron preload.js 시그니처를 Tauri invoke/listen 으로 구현.
// - Electron 에서 로드되면(window.api 존재 or __TAURI__ 부재) 아무 것도 하지 않는다.
// - Tauri 에서는 __TAURI__.core.invoke / __TAURI__.event.listen 을 사용한다 (withGlobalTauri true).
(function () {
  // Electron preload 가 이미 window.api 를 제공하면 절대 덮어쓰지 않는다.
  if (window.api) return;
  var T = window.__TAURI__;
  if (!T || !T.core || !T.event) return; // Tauri 런타임이 아님

  var invoke = T.core.invoke;
  var listen = T.event.listen;

  // 오디오 자동재생 폴백: WKWebView/WebView2 에서 제스처 없이 소리가 나도록
  // AudioContext 를 생성 즉시 resume 하고, Rust eval 이 호출할 수 있는 훅을 남긴다.
  try {
    var AC = window.AudioContext || window.webkitAudioContext;
    if (AC && !window.__maxalertAudioPatched) {
      window.__maxalertAudioPatched = true;
      window.__maxalertAudioCtxs = [];
      var Patched = function () {
        var ctx = new AC(arguments[0]);
        window.__maxalertAudioCtxs.push(ctx);
        try { ctx.resume(); } catch (e) {}
        return ctx;
      };
      Patched.prototype = AC.prototype;
      window.AudioContext = Patched;
      window.webkitAudioContext = Patched;
      window.__maxalertResumeAudio = function () {
        (window.__maxalertAudioCtxs || []).forEach(function (c) {
          try { c.resume(); } catch (e) {}
        });
      };
    }
  } catch (e) {}

  window.api = {
    listTodos: function () { return invoke('todos_list'); },
    addTodo: function (t) { return invoke('todos_add', { payload: t }); },
    updateTodo: function (id, patch) { return invoke('todos_update', { id: id, patch: patch }); },
    toggleTodo: function (id) { return invoke('todos_toggle', { id: id }); },
    deleteTodo: function (id) { return invoke('todos_delete', { id: id }); },
    postponeTodo: function (id, minutes) { return invoke('todos_postpone', { id: id, minutes: minutes }); },
    getSettings: function () { return invoke('settings_get'); },
    setSettings: function (s) { return invoke('settings_set', { patch: s }); },
    syncNotion: function () { return invoke('notion_sync'); },
    listNotionUsers: function () { return invoke('notion_users'); },
    copyReport: function () { return invoke('report_copy'); },
    buyTheme: function (id) { return invoke('shop_buy_theme', { id: id }); },
    openDashboard: function () { return invoke('dashboard_open'); },
    quitApp: function () { return invoke('app_quit'); },
    closeReward: function () { return invoke('reward_close'); },
    setMouseThrough: function (ignore) { return invoke('postit_mouse', { ignore: !!ignore }); },
    dragStart: function () { return invoke('postit_drag_start'); },
    dragEnd: function () { return invoke('postit_drag_end'); },
    onTodos: function (cb) { listen('todos', function (e) { cb(e.payload); }); },
    onSirenTodo: function (cb) { listen('siren:todo', function (e) { cb(e.payload); }); }
  };
})();
