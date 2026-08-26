// 悬浮面板小窗:按主条发来的 payload 渲染地图列表 / 药水图标格 / 时间选择器,点击回传选择
(() => {
  'use strict';

  const root = document.getElementById('popup-root');

  // 小药水瓶图标(颜色由主条按药水类型给出,图片缺失时回退)
  const potionSvg = (color) => `
    <svg viewBox="0 0 20 20" width="17" height="17">
      <rect x="8.2" y="0.8" width="3.6" height="2.4" rx="1" fill="#b98a56"/>
      <rect x="7.6" y="2.8" width="4.8" height="3.2" fill="#cfc8bb" opacity=".55"/>
      <rect x="4.4" y="6" width="11.2" height="12.2" rx="4.2" fill="${color}"/>
      <ellipse cx="7.2" cy="10.6" rx="1.2" ry="3.6" fill="#fff" opacity=".28"/>
    </svg>`;

  function renderMap(p) {
    if (!p.rows.length) {
      root.innerHTML = '<div class="popup-empty">没有匹配的地图</div>';
      return;
    }
    p.rows.forEach((row, i) => {
      const div = document.createElement('div');
      div.className = 'popup-opt' + (i === p.cursor ? ' active' : '');
      div.textContent = row.name;
      if (row.street) {
        const s = document.createElement('span');
        s.className = 'popup-opt-street';
        s.textContent = ` · ${row.street}`;
        div.appendChild(s);
      }
      div.addEventListener('mousedown', (e) => {
        e.preventDefault(); // 不抢主条焦点
        // 回传 mapid 而非行号:主条快速输入时列表可能已刷新,行号会指向另一张地图
        window.mxdApi.popup.pick({ kind: 'map', mapid: row.mapid });
      });
      root.appendChild(div);
    });
    if (p.cursor >= 0) {
      const el = root.children[p.cursor];
      if (el) el.scrollIntoView({ block: 'nearest' });
    }
  }

  function renderPotion(p) {
    const grid = document.createElement('div');
    grid.className = 'popup-grid';
    p.items.forEach((it, i) => {
      const btn = document.createElement('button');
      btn.className = 'popup-potion' + (it.selected ? ' active' : '');
      btn.title = it.name;
      const img = document.createElement('img');
      img.src = it.icon;
      img.onerror = () => { btn.innerHTML = potionSvg(it.color); }; // 图标缺失时回退
      btn.appendChild(img);
      btn.addEventListener('mousedown', (e) => {
        e.preventDefault();
        // 回传 key+itemid 而非行号:主条可能已因失焦清空打开上下文,行号/上下文都会失效
        window.mxdApi.popup.pick({ kind: 'potion', key: p.key, itemid: it.itemid });
      });
      grid.appendChild(btn);
    });
    root.appendChild(grid);
  }

  // 时分(秒)输入器(999打卡 / 神秘商人共用):999 时:分;商人 withSeconds 时:分:秒
  function renderTime(p) {
    const wrap = document.createElement('div');
    wrap.className = 'popup-time';
    const title = document.createElement('div');
    title.className = 'popup-time-title';
    title.textContent = p.title || '打卡时间';
    wrap.appendChild(title);

    const pad = (n) => String(n).padStart(2, '0');
    const row = document.createElement('div');
    row.className = 'popup-time-row';
    const makeInput = (val) => {
      const inp = document.createElement('input');
      inp.className = 'popup-time-input';
      inp.type = 'text';
      inp.inputMode = 'numeric';
      inp.autocomplete = 'off';
      inp.value = pad(val);
      return inp;
    };
    const hourIn = makeInput(p.hour);
    const minIn = makeInput(p.minute);
    const secIn = p.withSeconds ? makeInput(p.second ?? 0) : null;
    const makeColon = () => {
      const c = document.createElement('span');
      c.className = 'popup-time-colon';
      c.textContent = ':';
      return c;
    };
    row.append(hourIn, makeColon(), minIn);
    if (secIn) row.append(makeColon(), secIn);
    wrap.appendChild(row);

    // 只允许数字;时输满两位跳到分,分输满两位跳到秒;点击默认全选
    [hourIn, minIn, secIn].filter(Boolean).forEach((inp) => {
      inp.addEventListener('input', () => {
        inp.value = inp.value.replace(/\D/g, '').slice(0, 2);
        if (inp === hourIn && inp.value.length === 2) minIn.focus();
        else if (inp === minIn && inp.value.length === 2 && secIn) secIn.focus();
      });
      inp.addEventListener('focus', () => setTimeout(() => inp.select(), 0));
    });

    const invalid = () => {
      row.classList.remove('err');
      void row.offsetWidth; // 重新触发动画
      row.classList.add('err');
      hourIn.focus();
      hourIn.select();
    };
    const sendOk = () => {
      const hour = +hourIn.value;
      const minute = +minIn.value;
      const second = secIn ? (+secIn.value || 0) : 0; // 秒留空按 0 处理
      if (!hourIn.value || !minIn.value || hour > (p.maxHour ?? 23) || minute > 59 || second > 59) { invalid(); return; }
      window.mxdApi.popup.pick({ kind: 'time', action: 'ok', hour, minute, second });
    };
    const keydown = (e) => {
      if (e.key === 'Enter') { e.preventDefault(); sendOk(); }
      else if (e.key === 'Escape') { e.preventDefault(); window.mxdApi.popup.pick({ kind: 'time', action: 'cancel' }); }
    };
    hourIn.addEventListener('keydown', keydown);
    minIn.addEventListener('keydown', keydown);

    const btns = document.createElement('div');
    btns.className = 'popup-time-btns';
    const ok = document.createElement('button');
    ok.textContent = '确定';
    ok.addEventListener('mousedown', (e) => { e.preventDefault(); sendOk(); });
    const cancel = document.createElement('button');
    cancel.textContent = '取消';
    cancel.addEventListener('mousedown', (e) => {
      e.preventDefault();
      window.mxdApi.popup.pick({ kind: 'time', action: 'cancel' });
    });
    btns.append(ok, cancel);
    wrap.appendChild(btns);
    root.appendChild(wrap);
    hourIn.focus();
    hourIn.select();
  }

  const render = (p) => {
    root.innerHTML = '';
    if (!p) return;
    if (p.kind === 'map') renderMap(p);
    else if (p.kind === 'potion') renderPotion(p);
    else if (p.kind === 'time') renderTime(p);
  };
  window.mxdApi.popup.onRender(render);
  // 面板加载前主条可能已发过渲染请求(Tauri 下事件会丢失)→ 主动拉取积压 payload
  Promise.resolve(window.mxdApi.popup.ready())
    .then((p) => { if (p) render(p); })
    .catch(() => { /* Electron 旧版无此接口,忽略 */ });
})();
