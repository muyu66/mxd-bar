// Tauri 桥接:基于 __TAURI_INTERNALS__ 内部 API(2.11 起已移除 withGlobalTauri 配置项),
// 保持与旧 Electron preload 完全相同的 window.mxdApi 接口
(() => {
  'use strict';
  const internals = window.__TAURI_INTERNALS__;
  if (!internals) return; // Electron 环境下由 preload.js 提供 mxdApi,此处不动

  const invoke = (cmd, args) => internals.invoke(cmd, args || {});
  // 与 @tauri-apps/api 的 event.listen 等价:注册 transformCallback 并监听 plugin:event
  // (2.11 的 listen 命令必须带 target,默认监听全部目标)
  const listen = (event, cb) =>
    invoke('plugin:event|listen', {
      event,
      target: { kind: 'Any' },
      handler: internals.transformCallback(cb),
    }).then((eventId) => () => invoke('plugin:event|unlisten', { eventId }));

  window.mxdApi = {
    getDeviceId: () => invoke('get_device_id'),
    getData: () => invoke('get_data'),
    submitRecord: (payload) => invoke('submit_record', { payload }),
    openSite: (url) => invoke('open_site', { url }),
    closeApp: () => invoke('close_app'),
    getSettings: () => invoke('get_settings'),
    saveSettings: (patch) => invoke('save_settings', { patch }),
    dragStart: () => invoke('drag_start'),
    dragMove: (dx, dy) => invoke('drag_move', { dx, dy }),
    dragEnd: () => invoke('drag_end'),
    // 悬浮面板(主条窗口用 show/render/close/onPick,面板小窗用 onRender/pick/ready)
    popup: {
      show: (opts) => invoke('popup_show', { opts }),
      render: (payload) => invoke('popup_render', { payload }),
      close: () => invoke('popup_close'),
      onPick: (cb) => listen('popup-picked', (e) => cb(e.payload)),
      onRender: (cb) => listen('popup-render', (e) => cb(e.payload)),
      pick: (data) => invoke('popup_pick', { data }),
      // 面板加载完成后取走期间积压的渲染请求(主条可能在面板加载前就发过 render)
      ready: () => invoke('popup_ready'),
    },
  };
})();
