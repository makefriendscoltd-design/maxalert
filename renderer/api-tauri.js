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
  var postitCursorCallbacks = [];

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
    postponeTodoToNextWeekday: function (id) { return invoke('todos_postpone_next_weekday', { id: id }); },
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
    onPostitCursor: function (cb) {
      if (typeof cb !== 'function') return function () {};
      postitCursorCallbacks.push(cb);
      return function () {
        var i = postitCursorCallbacks.indexOf(cb);
        if (i !== -1) postitCursorCallbacks.splice(i, 1);
      };
    },
    onSirenTodo: function (cb) { listen('siren:todo', function (e) { cb(e.payload); }); }
  };

  // ---- 업데이트 안내 (Tauri 대시보드 전용) ----
  var updateDismissKey = 'maxUpdateDismissed';
  var updateDownloadUrl = 'https://lounge.aimax.ai.kr';

  var findDashboardInsertPoint = function () {
    if (location.pathname.indexOf('dashboard') === -1 || !document.body) return null;
    var wrap = document.querySelector('body > .wrap');
    if (wrap) return { parent: wrap, before: wrap.firstChild };
    for (var i = 0; i < document.body.children.length; i += 1) {
      var child = document.body.children[i];
      if (child.tagName !== 'SCRIPT' && child.tagName !== 'STYLE' && child.tagName !== 'SVG') {
        return { parent: document.body, before: child };
      }
    }
    return null;
  };

  var showUpdateBanner = function (version) {
    version = String(version || '').trim();
    if (!version || location.pathname.indexOf('dashboard') === -1) return;
    try {
      if (window.localStorage.getItem(updateDismissKey) === version) return;
    } catch (e) {}

    var insertPoint = findDashboardInsertPoint();
    if (!insertPoint) return;
    var oldBanner = document.getElementById('max-update-banner');
    if (oldBanner) {
      if (oldBanner.getAttribute('data-version') === version) return;
      oldBanner.parentNode.removeChild(oldBanner);
    }

    if (!document.getElementById('max-update-banner-style')) {
      var style = document.createElement('style');
      style.id = 'max-update-banner-style';
      style.textContent =
        '#max-update-banner{display:flex;align-items:center;gap:12px;width:100%;margin:0 0 14px;padding:11px 13px;border:1px solid rgba(96,165,250,.38);border-radius:12px;background:linear-gradient(135deg,rgba(20,28,46,.94),rgba(30,34,46,.88));color:#e9ecf4;box-shadow:0 10px 28px rgba(0,0,0,.22);-webkit-backdrop-filter:blur(12px);backdrop-filter:blur(12px);font:600 13px/1.45 system-ui,sans-serif}' +
        '#max-update-banner .max-update-message{flex:1;min-width:0}' +
        '#max-update-banner button{border:0;font:inherit;cursor:pointer}' +
        '#max-update-banner .max-update-download{padding:0;background:transparent;color:#60a5fa;font-weight:800;text-decoration:underline;text-underline-offset:3px}' +
        '#max-update-banner .max-update-close{width:26px;height:26px;flex:0 0 26px;border:1px solid rgba(140,160,255,.18);border-radius:7px;background:rgba(13,15,20,.48);color:#8a93a8;font-size:12px;line-height:1}' +
        '#max-update-banner .max-update-download:hover{color:#93c5fd}' +
        '#max-update-banner .max-update-close:hover{color:#e9ecf4;border-color:rgba(96,165,250,.45)}';
      document.head.appendChild(style);
    }

    var banner = document.createElement('div');
    banner.id = 'max-update-banner';
    banner.setAttribute('data-version', version);
    banner.setAttribute('role', 'status');
    banner.setAttribute('aria-live', 'polite');

    var message = document.createElement('span');
    message.className = 'max-update-message';
    message.appendChild(document.createTextNode('새 버전 v' + version + ' 이 나왔어요 — '));
    var download = document.createElement('button');
    download.type = 'button';
    download.className = 'max-update-download';
    download.textContent = '다운로드';
    download.addEventListener('click', function () {
      try {
        var request = invoke('open_external', { url: updateDownloadUrl });
        if (request && typeof request.catch === 'function') request.catch(function () {});
      } catch (e) {}
    });
    message.appendChild(download);

    var close = document.createElement('button');
    close.type = 'button';
    close.className = 'max-update-close';
    close.textContent = 'X';
    close.setAttribute('aria-label', '업데이트 알림 닫기');
    close.addEventListener('click', function () {
      try { window.localStorage.setItem(updateDismissKey, version); } catch (e) {}
      if (banner.parentNode) banner.parentNode.removeChild(banner);
    });

    banner.appendChild(message);
    banner.appendChild(close);
    insertPoint.parent.insertBefore(banner, insertPoint.before);
  };

  var onUpdateAvailable = function (e) {
    var payload = e && e.payload ? e.payload : {};
    var render = function () { showUpdateBanner(payload.version); };
    if (document.readyState === 'loading') {
      document.addEventListener('DOMContentLoaded', render, { once: true });
    } else {
      render();
    }
  };
  try {
    var updateListener = listen('update:available', onUpdateAvailable);
    if (updateListener && typeof updateListener.catch === 'function') {
      updateListener.catch(function () {});
    }
  } catch (e) {}

  // ---- 포스트잇 클릭스루 제어 (Tauri 전용) ----
  // macOS 는 ignore_cursor_events 상태에서 mousemove 를 웹뷰에 전달하지 않아
  // (Electron forward:true 부재) postit.html 의 hover 해제 로직이 실행될 기회가 없다.
  // Rust 가 postit:cursor 로 창-로컬 논리좌표를 밀어주면, 여기서 postit.html 과
  // 동일한 규칙(.interactive closest)으로 판정해 상태 변화 시에만 토글한다.
  if (location.pathname.indexOf('postit') !== -1) {
    var through = true;
    listen('postit:cursor', function (e) {
      var p = e.payload || {};
      var el = (p.x >= 0 && p.y >= 0) ? document.elementFromPoint(p.x, p.y) : null;
      var interactive = !!(el && el.closest && el.closest('.interactive'));
      // 포커스 확장 트리거는 +N 배지 위에서만 — hero 조준 중 축소 방지 (postit.html 과 동일 규칙)
      var expandTrigger = !!(el && el.closest && el.closest('.focus-more'));
      postitCursorCallbacks.slice().forEach(function (cb) {
        try { cb({ x: p.x, y: p.y, interactive: interactive, expandTrigger: expandTrigger }); } catch (err) {}
      });
      if (interactive && through) {
        through = false;
        invoke('postit_mouse', { ignore: false });
      } else if (!interactive && !through) {
        through = true;
        invoke('postit_mouse', { ignore: true });
      }
    });
  }

  // ---- 시간 입력 보정 (Tauri 전용) ----
  // WKWebView 의 input[type=time] 은 UI/값 반영이 불완전해 사용자가 시간을 골라도
  // value 가 빈 문자열로 남는다 (실데이터로 확인: dueAt null). Tauri 런타임에서는
  // 예측 가능한 텍스트 입력으로 강제 전환하고 HH:MM 으로 정규화한다.
  // Electron(Chromium) 은 이 래퍼가 no-op 이므로 네이티브 타임 피커를 유지한다.
  if (location.pathname.indexOf('dashboard') !== -1) {
    var fixTimeInput = function () {
      var input = document.getElementById('inTime');
      if (!input || input.dataset.maxalertTimeFixed) return;
      input.dataset.maxalertTimeFixed = '1';
      input.type = 'text';
      input.placeholder = 'HH:MM';
      input.style.width = '5.5em';
      var normalize = function () {
        var v = input.value.trim();
        if (!v) return;
        var h, m;
        if (v.indexOf(':') !== -1) {
          var parts = v.split(':');
          h = parseInt(parts[0], 10);
          m = parseInt(parts[1], 10);
        } else {
          var digits = v.replace(/[^0-9]/g, '');
          if (digits.length === 3) { h = parseInt(digits.slice(0, 1), 10); m = parseInt(digits.slice(1), 10); }
          else if (digits.length === 4) { h = parseInt(digits.slice(0, 2), 10); m = parseInt(digits.slice(2), 10); }
          else if (digits.length >= 1 && digits.length <= 2) { h = parseInt(digits, 10); m = 0; }
          else { input.value = ''; return; }
        }
        if (isNaN(h) || isNaN(m) || h > 23 || m > 59 || h < 0 || m < 0) { input.value = ''; return; }
        input.value = (h < 10 ? '0' + h : '' + h) + ':' + (m < 10 ? '0' + m : '' + m);
      };
      input.addEventListener('blur', normalize);
      input.addEventListener('change', normalize);
      input.addEventListener('keydown', function (ev) { if (ev.key === 'Enter') normalize(); });
    };
    if (document.readyState === 'loading') {
      document.addEventListener('DOMContentLoaded', fixTimeInput);
    } else {
      fixTimeInput();
    }
  }
})();
