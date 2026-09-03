// 冒险岛怀旧服经验记录器 — 渲染进程
// 4 态状态机:输入BAR → 计时器 → 结束BAR → 成功动画
// 地图搜索与药水/时分选择通过悬浮面板(独立小窗)完成,不改变主条布局
(() => {
  'use strict';

  const $ = (sel) => document.querySelector(sel);

  let DATA = null;          // { expTable, maps, potions, jobs }
  let deviceId = '';
  let expMode = 'value';    // 'percent' | 'value'
  let partyMode = 'solo';   // 'solo' | 'party'
  let selectedMap = null;   // { mapid, mapName, scene }
  let startSnapshot = null;
  let startTs = 0;
  let stopTs = 0;
  let stopActiveMs = 0;      // 有效计时时长(排除暂停时段),用于上报
  let elapsedBase = 0;       // 暂停前的累计计时
  let pausedAt = 0;          // 暂停时刻;0 = 未暂停
  let timerInterval = null;
  let submitting = false;

  // 吸顶/收缩:拖到屏幕顶部附近 → Rust 贴顶吸附;鼠标离开 3 秒收缩成粗线,悬停还原
  let snapOn = false;          // Rust 判定:窗口贴合工作区顶部
  let collapsed = false;       // 已收缩成粗线(仅吸顶态)
  let hovered = false;         // 鼠标在窗口内
  let collapseTimer = null;
  let currentState = 'input';
  const COLLAPSE_DELAY = 3000;

  // 999打卡 / 神秘商人 / BOSS计时(时间戳基准,关闭程序时间照走)
  let checkin999Anchor = null;   // 上次999打卡时刻(ms)
  let merchantAnchor = null;     // 上次神秘商人刷新时刻(ms)
  let bossAnchor = null;         // 上次BOSS死亡时刻(ms)
  let timePickerTarget = null;   // '999' | 'merchant'
  let merchantRefreshed = false;    // 已跨过刷新点 → 显示"已刷新"+振动,悬停确认后清除
  let merchantHovered = false;      // 鼠标悬停在商人框上
  let merchantLastRemaining = null; // 上一拍剩余(ms),用于检测周期翻转
  const HOUR_MS = 3600 * 1000;
  const DAY_MS = 24 * HOUR_MS;
  const MERCHANT_MS = 6 * HOUR_MS;

  const fmt = (n) => Math.round(n).toLocaleString('zh-CN');
  const clampLevel = (v) => Math.min(200, Math.max(1, Math.round(v) || 1));
  const perLevelExp = (level) => (DATA.expTable.perLevel[level - 1] || 0); // 200 级返回 0

  // ---------- 初始化 ----------
  async function init() {
    DATA = await window.mxdApi.getData();
    deviceId = await window.mxdApi.getDeviceId();
    populateJobs();
    populatePotions();
    await applySavedSettings();
    bindEvents();
    // 吸顶/收缩初始状态(上次停在顶部 → 启动即恢复吸顶,3 秒后照常收缩)
    try {
      const snap = await window.mxdApi.getSnapState();
      snapOn = snap.snapped;
      setCollapsedClass(snap.collapsed);
      armCollapse();
    } catch (e) { /* 桥接缺失时忽略 */ }
    refreshExpUI();
    refreshPartyUI();
    updateTimers();
    setInterval(updateTimers, 500); // 打卡/商人/BOSS计时刷新
    fitWindowWidth();
  }

  // ---------- 面板宽度贴合内容 ----------
  // 量取输入条各控件实际宽度 + 间距,让窗口紧凑包裹(DPI 换算在 Rust 侧完成)
  function fitWindowWidth() {
    const bar = $('#state-input .bar');
    if (!bar || !window.mxdApi.setWindowWidth) return;
    const items = [...bar.children];
    const content = items.reduce((s, el) => s + el.getBoundingClientRect().width, 0)
      + Math.max(0, items.length - 1) * 10; // gap: 10px(见 style.css .bar)
    window.mxdApi.setWindowWidth(content + 44); // 左右 padding 各 22px
  }

  // ---------- 设置恢复(上次关闭时的状态) ----------
  async function applySavedSettings() {
    let saved = {};
    try { saved = await window.mxdApi.getSettings(); } catch (e) { /* 读取失败用默认 */ }
    if (!saved || typeof saved !== 'object') return;
    if (saved.level) $('#in-level').value = clampLevel(saved.level);
    if (saved.outLevel) $('#out-level').value = clampLevel(saved.outLevel);
    if (saved.job) {
      const opt = [...$('#in-job').options].find((o) => o.value === saved.job);
      if (opt) $('#in-job').value = saved.job;
    }
    ['in-hp', 'in-mp', 'out-hp', 'out-mp'].forEach((key) => {
      const id = saved.potions && saved.potions[key];
      if (!id) return;
      const p = findPot(potKindOf(key), id);
      if (p) {
        potionSel[key] = p;
        setPotionBtn($(`#${key}-potion-btn`), p);
      }
    });
    if (saved.expMode === 'percent' || saved.expMode === 'value') expMode = saved.expMode;
    if (saved.partyMode === 'party' || saved.partyMode === 'solo') partyMode = saved.partyMode;
    if (saved.map && saved.map.mapid != null) {
      selectedMap = saved.map;
      $('#in-map').value = saved.map.mapName;
    }
    if (typeof saved.checkin999 === 'number') checkin999Anchor = saved.checkin999;
    if (typeof saved.merchant === 'number') merchantAnchor = saved.merchant;
    if (typeof saved.boss === 'number') bossAnchor = saved.boss;
  }

  // ---------- 设置持久化 ----------
  function persistSettings() {
    window.mxdApi.saveSettings({
      level: $('#in-level').value,
      outLevel: $('#out-level').value,
      job: $('#in-job').value,
      map: selectedMap || null,
      potions: {
        'in-hp': potionSel['in-hp'] ? potionSel['in-hp'].itemid : null,
        'in-mp': potionSel['in-mp'] ? potionSel['in-mp'].itemid : null,
        'out-hp': potionSel['out-hp'] ? potionSel['out-hp'].itemid : null,
        'out-mp': potionSel['out-mp'] ? potionSel['out-mp'].itemid : null,
      },
      expMode,
      partyMode,
      checkin999: checkin999Anchor,
      merchant: merchantAnchor,
      boss: bossAnchor,
    });
  }

  // ---------- 职业下拉框 ----------
  function populateJobs() {
    const sel = $('#in-job');
    for (const g of DATA.jobs) {
      const og = document.createElement('optgroup');
      og.label = g.group;
      for (const name of g.jobs) {
        const o = document.createElement('option');
        o.value = name;
        o.textContent = name;
        og.appendChild(o);
      }
      sel.appendChild(og);
    }
  }

  // ---------- 药水:图标按钮 + 悬浮图标格 ----------
  const POTS = {};  // { hp: [...], mp: [...] }
  const potionSel = { 'in-hp': null, 'in-mp': null, 'out-hp': null, 'out-mp': null };
  let potionPickerCtx = null; // { key, btnId, items }

  function potionLabel(p) {
    if (p.full) return `${p.name} (HP/MP 全满)`;
    const parts = [];
    if (p.hpPct && p.mpPct) parts.push(`HP+${p.hpPct}% MP+${p.mpPct}%`);
    else {
      if (p.hp) parts.push(`HP+${fmt(p.hp)}`);
      if (p.mp) parts.push(`MP+${fmt(p.mp)}`);
    }
    return `${p.name} (${parts.join(' ')})`;
  }

  function potionColor(p) {
    if (p.full) return '#f5a623';
    if (p.hpPct && p.mpPct) return '#b07fe0';
    if (p.hp) return '#e0617a';
    return '#4a90d9';
  }

  // 药水图标:Tauri 版内嵌为 data URL,Electron 版拼 file:// 路径(加载失败时回退自绘 SVG)
  const potionIconUrl = (itemid) => {
    if (DATA.icons && DATA.icons[itemid]) return DATA.icons[itemid];
    return `file:///${encodeURI(String(DATA.potionIconDir || '').replace(/\\/g, '/'))}/${itemid}.png`;
  };

  // 小药水瓶图标(与悬浮面板一致)
  const potionSvg = (color) => `
    <svg viewBox="0 0 20 20" width="17" height="17">
      <rect x="8.2" y="0.8" width="3.6" height="2.4" rx="1" fill="#b98a56"/>
      <rect x="7.6" y="2.8" width="4.8" height="3.2" fill="#cfc8bb" opacity=".55"/>
      <rect x="4.4" y="6" width="11.2" height="12.2" rx="4.2" fill="${color}"/>
      <ellipse cx="7.2" cy="10.6" rx="1.2" ry="3.6" fill="#fff" opacity=".28"/>
    </svg>`;

  function setPotionBtn(btn, p) {
    btn.innerHTML = '';
    const img = document.createElement('img');
    img.src = potionIconUrl(p.itemid);
    img.onerror = () => { btn.innerHTML = potionSvg(potionColor(p)); }; // 图标缺失时回退
    btn.appendChild(img);
    btn.title = potionLabel(p);
  }

  function findPot(kind, itemId) {
    return POTS[kind].find((p) => p.itemid === itemId) || POTS[kind][0];
  }

  const potKindOf = (key) => (key.endsWith('-hp') ? 'hp' : 'mp');

  function populatePotions() {
    POTS.hp = DATA.potions.filter((p) => p.full || p.hp || p.hpPct);
    POTS.mp = DATA.potions.filter((p) => p.full || p.mp || p.mpPct);
    ['in-hp', 'in-mp', 'out-hp', 'out-mp'].forEach((key) => {
      potionSel[key] = POTS[potKindOf(key)][0];
      setPotionBtn($(`#${key}-potion-btn`), potionSel[key]);
    });
  }

  // ---------- 悬浮面板(地图搜索 / 药水 / 时分选择器共用独立小窗) ----------
  let popupOpen = null;      // null | 'map' | 'potion' | 'time'
  let mapJustPicked = false; // 刚选中地图:吸收随后 Chromium 焦点恢复(见 focus 事件)
  let mapCursor = -1;
  let currentMapList = [];
  let lastMapRowCount = -1;  // 面板当前行数,变化时才重新定位/定高(每键不重定位)

  function closePopup() {
    if (!popupOpen) return;
    popupOpen = null;
    potionPickerCtx = null;
    window.mxdApi.popup.close();
    armCollapse(); // 面板占用期间不收缩,关闭后重新计时
  }

  // --- 地图面板 ---
  function mapFilter(q) {
    const kw = (q || '').trim().toLowerCase();
    if (!kw) return DATA.maps.slice(0, 60);
    // 排序:精确匹配 > 前缀匹配 > 其他包含(搜"南港"时"南港"排第一,而不是"岔道（通往南港和彩虹村）")
    const rank = (m) => {
      const name = m.mapName.toLowerCase();
      if (name === kw) return 0;
      if (name.startsWith(kw)) return 1;
      return 2;
    };
    return DATA.maps
      .filter((m) => m.mapName.toLowerCase().includes(kw)
        || String(m.mapid).includes(kw)
        || (m.street || '').toLowerCase().includes(kw)) // 支持按区域搜(迷宫/隐藏地图…)
      .sort((a, b) => rank(a) - rank(b))
      .slice(0, 60);
  }

  function renderMapPopup() {
    currentMapList = mapFilter($('#in-map').value);
    window.mxdApi.popup.render({
      kind: 'map',
      // 只显示名字 + 所属区域(如"龙族打猎场 · 隐藏地图"),区域为空时省略;
      // mapid 随行下发,点击按 mapid 回传而非行号 → 快速输入期间列表刷新也不会选错地图
      rows: currentMapList.map((m) => ({ name: m.mapName, street: m.street || '', mapid: m.mapid })),
      cursor: mapCursor,
    });
    // 面板高度随行数伸缩(空列表也有一行提示),上限 300;行数没变不动窗口,避免每键重定位
    const rows = Math.max(1, currentMapList.length);
    if (rows !== lastMapRowCount) {
      lastMapRowCount = rows;
      window.mxdApi.popup.show({
        anchorX: $('#in-map').getBoundingClientRect().left,
        width: 220,
        height: Math.min(300, 8 + rows * 26),
      });
    }
  }

  function openMapPopup() {
    popupOpen = 'map';
    lastMapRowCount = -1; // 强制重新定位/定高(兜底:面板可能已被 Rust 侧自动收起)
    renderMapPopup();
  }

  function chooseMap(m) {
    selectedMap = m;
    mapJustPicked = true;
    // 选中后 Chromium 会无 mousedown 地把焦点还给输入框 → 短暂窗口内吸收,过期即真实操作
    setTimeout(() => { mapJustPicked = false; }, 300);
    $('#in-map').value = m.mapName;
    closePopup();
    $('#in-map').blur();
    persistSettings(); // 记住地图,返回/重启不用重新输入
  }

  function moveMapCursor(delta) {
    if (!currentMapList.length) return;
    mapCursor = Math.min(currentMapList.length - 1, Math.max(0, mapCursor + delta));
    renderMapPopup();
  }

  function chooseCursorMap() {
    // 未用方向键移动过光标时(直接输入+回车),默认选第一个匹配项
    const idx = mapCursor >= 0 ? mapCursor : (currentMapList.length ? 0 : -1);
    if (idx >= 0 && currentMapList[idx]) chooseMap(currentMapList[idx]);
  }

  // --- 药水面板 ---
  function openPotionPopup(btn) {
    if (popupOpen === 'potion' && potionPickerCtx && potionPickerCtx.btnId === btn.id) {
      closePopup();
      return;
    }
    const key = btn.id.replace('-potion-btn', ''); // e.g. 'in-hp'
    const items = POTS[btn.dataset.list];
    potionPickerCtx = { key, btnId: btn.id, items };
    popupOpen = 'potion';
    const r = btn.getBoundingClientRect();
    const cols = 8;
    const rows = Math.ceil(items.length / cols);
    window.mxdApi.popup.show({
      anchorX: r.left,
      width: 16 + cols * 30 + (cols - 1) * 4,
      height: 12 + rows * 34,
    });
    window.mxdApi.popup.render({
      kind: 'potion',
      key, // 点击时原样回传(见 popup.js),主条据此定位按钮,不依赖打开时的上下文
      items: items.map((p) => ({
        itemid: p.itemid, // 点击时原样回传(见 popup.js pick),主条据此定位药水
        name: potionLabel(p),
        icon: potionIconUrl(p.itemid),
        color: potionColor(p),
        selected: potionSel[key] === p,
      })),
    });
  }

  // ---------- 吸顶 / 收缩 ----------
  // 仅"待开始记录"状态可收缩;悬浮面板打开(锚在主条下方)时也不收缩
  function collapsible() {
    return currentState === 'input' && popupOpen === null;
  }

  function clearCollapseTimer() {
    if (collapseTimer) { clearTimeout(collapseTimer); collapseTimer = null; }
  }

  // 条件齐备则 3 秒后收缩;期间悬停/切状态/开面板都会取消
  function armCollapse() {
    clearCollapseTimer();
    if (!snapOn || !collapsible() || hovered || collapsed) return;
    collapseTimer = setTimeout(() => {
      collapseTimer = null;
      if (snapOn && collapsible() && !hovered && !collapsed) {
        window.mxdApi.setCollapsed(true); // Rust 侧复核鼠标位置后执行收缩并发回事件
      }
    }, COLLAPSE_DELAY);
  }

  // 收缩/还原的最终形态以 Rust 事件为准(离开吸顶时 Rust 强制还原,这里只同步样式)
  function setCollapsedClass(c) {
    collapsed = c;
    document.body.classList.toggle('collapsed', c);
    if (c) closePopup();
  }

  // ---------- 999打卡 / 神秘商人 / BOSS计时 倒计时 ----------
  // 时间戳 → 本地时区当天时分秒(epoch 按 UTC 午夜对齐,% DAY_MS 截取会偏 8 小时)
  function formatClockOf(ts) {
    const d = new Date(ts);
    return [d.getHours(), d.getMinutes(), d.getSeconds()]
      .map((n) => String(n).padStart(2, '0')).join(':');
  }

  // BOSS已过时长:60 秒内显示秒,100 分钟内显示分钟,再长封顶 99+ 分钟
  function formatElapsed(ms) {
    if (ms < 60 * 1000) return `${Math.floor(ms / 1000)} 秒钟`;
    if (ms < 100 * 60 * 1000) return `${Math.floor(ms / 60000)} 分钟`;
    return '99+ 分钟';
  }

  function updateTimers() {
    const el999 = $('#checkin-999');
    const elM = $('#checkin-merchant');
    if (!el999 || !elM) return; // 非输入态不显示
    // 999打卡:距下次可打卡 = 上次打卡 + 24h - now;到点转红色待打卡
    if (!checkin999Anchor || checkin999Anchor + DAY_MS <= Date.now()) {
      el999.textContent = '待打卡';
      el999.classList.add('waiting');
    } else {
      el999.classList.remove('waiting');
      // 锚点恰好等于当前时刻时剩余为 24h 整,floor 会显示 24:00:00,clamp 保证从 23:59:59 起
      el999.textContent = formatHMS(Math.min(checkin999Anchor + DAY_MS - Date.now(), DAY_MS - 1));
    }
    // 神秘商人:6 小时周期循环倒计时。倒计时结束(周期翻转)时振动并显示"已刷新",
    // 内部周期照常继续;鼠标移到该框 → 停止振动并恢复倒计时(ceil 取整,不显示 00:00:00)
    if (!merchantAnchor) {
      merchantLastRemaining = null;
      elM.textContent = '--:--:--';
      elM.classList.add('idle');
    } else {
      elM.classList.remove('idle');
      const remaining = MERCHANT_MS - ((Date.now() - merchantAnchor) % MERCHANT_MS);
      // 上一拍接近 0 且这一拍接近整周期 = 刚跨过刷新点
      if (merchantLastRemaining !== null && merchantLastRemaining < 2000 && remaining > MERCHANT_MS - 2000) {
        if (!merchantHovered) merchantRefreshed = true;
      }
      merchantLastRemaining = remaining;
      if (merchantRefreshed && !merchantHovered) {
        elM.textContent = '已刷新';
        elM.classList.add('refreshed');
      } else {
        elM.classList.remove('refreshed');
        elM.textContent = formatHMS(Math.ceil(remaining));
      }
    }
    // BOSS计时:左标签 = 死亡时刻(当天时分秒),右标签 = 距死亡已过时长
    const elBD = $('#boss-death');
    const elBE = $('#boss-elapsed');
    if (!bossAnchor) {
      elBD.classList.add('idle');
      elBD.textContent = '--:--:--';
      elBE.classList.add('idle');
      elBE.textContent = '--';
    } else {
      elBD.classList.remove('idle');
      elBD.textContent = formatClockOf(bossAnchor);
      elBE.classList.remove('idle');
      elBE.textContent = formatElapsed(Math.max(0, Date.now() - bossAnchor)); // 时钟回拨时兜底为 0
    }
    // 收缩成粗线时:999 待打卡红闪 / 商人已刷新金闪,同时触发则红金交替闪
    if (document.body.classList.contains('collapsed')) {
      const mAlert = merchantRefreshed && !merchantHovered;
      const cAlert = el999.classList.contains('waiting');
      document.body.classList.toggle('alert-merchant', mAlert && !cAlert);
      document.body.classList.toggle('alert-999', cAlert && !mAlert);
      document.body.classList.toggle('alert-both', mAlert && cAlert);
    }
  }

  // 点击倒计时框 → 弹出时分(秒)选择器(999/商人默认规则见下,BOSS默认当前时间)
  function openTimePicker(target) {
    timePickerTarget = target;
    popupOpen = 'time';
    const box = target === '999' ? $('#checkin-999')
      : target === 'merchant' ? $('#checkin-merchant')
      : $('#boss-death');
    const r = box.getBoundingClientRect();
    const now = new Date();
    const isMerchant = target === 'merchant';
    const isBoss = target === 'boss';
    window.mxdApi.popup.show({ anchorX: r.left, width: isMerchant || isBoss ? 164 : 120, height: 100, focusable: true }); // 需要键盘输入
    window.mxdApi.popup.render({
      kind: 'time',
      // 999默认当前时分;神秘商人默认 05:59:59(6小时周期刚刷新时确认 → 从 05:59:59 起倒计时);BOSS默认当前时分秒
      hour: isMerchant ? 5 : now.getHours(),
      minute: isMerchant ? 59 : now.getMinutes(),
      second: isMerchant ? 59 : isBoss ? now.getSeconds() : 0,
      title: isMerchant ? '刷新剩余时间' : isBoss ? 'BOSS死亡时间' : '打卡时间',
      maxHour: isMerchant ? 5 : 23, // 商人周期 6 小时,剩余时长最多 5:59:59
      withSeconds: isMerchant || isBoss, // 商人剩余时长/BOSS死亡时刻精确到秒;999打卡只到分钟
    });
  }

  // 999打卡专用:选中的时分(一天中的时刻) → 锚定最近一次 ≤ now 的时刻(选未来时刻视为昨天同一时刻)。
  // 秒数取当前秒,保证"选当前时分→确认"的瞬间恰好从 23:59:59 开始倒计时。
  function timeAnchorOf(hour, minute) {
    const nowMs = Date.now();
    const now = new Date(nowMs);
    const t0 = new Date(now.getFullYear(), now.getMonth(), now.getDate(), hour, minute, 0, 0).getTime();
    const secOff = nowMs % 60000;
    return t0 <= nowMs ? t0 + secOff : t0 - DAY_MS + secOff;
  }

  // BOSS专用:选中的时分秒一律锚定当天(允许未来时刻:到点前距离按 0 计,到点后自动开始累计)
  function bossTimeAnchorOf(hour, minute, second) {
    const now = new Date();
    return new Date(now.getFullYear(), now.getMonth(), now.getDate(), hour, minute, second || 0, 0).getTime();
  }

  // ---------- EXP 双模式 ----------
  function expValueFromInput(level, mode, input) {
    if (mode === 'percent') {
      const need = perLevelExp(level);
      if (!need) return 0;
      return Math.round((input / 100) * need);
    }
    return Math.round(input);
  }

  function refreshExpUI() {
    // 两个 EXP 分段按钮同步当前模式(仅 EXP 的两个 seg)
    document.querySelectorAll('#seg-start, #seg-end').forEach((seg) => {
      seg.querySelectorAll('button').forEach((b) => {
        b.classList.toggle('active', b.dataset.mode === expMode);
      });
    });
    updateSegDisabled($('#seg-start'), clampLevel($('#in-level').value));
    updateSegDisabled($('#seg-end'), clampLevel($('#out-level').value));
  }

  function refreshPartyUI() {
    document.querySelectorAll('#seg-party button').forEach((b) => {
      b.classList.toggle('active', b.dataset.mode === partyMode);
    });
  }

  // 满级(200)时无法用百分比模式
  function updateSegDisabled(seg, level) {
    seg.querySelectorAll('button[data-mode="percent"]').forEach((b) => (b.disabled = level >= 200));
  }

  // ---------- 快照读取 ----------
  function readSnapshot(prefix) {
    const level = clampLevel($(`#${prefix}-level`).value);
    const hpItem = potionSel[`${prefix}-hp`] || POTS.hp[0];
    const mpItem = potionSel[`${prefix}-mp`] || POTS.mp[0];
    const input = parseFloat($(`#${prefix}-exp`).value) || 0;
    const count = (el) => Math.max(0, Math.round(parseFloat($(el).value) || 0));
    const goldOf = (el) => Math.max(0, Math.round((parseFloat($(el).value) || 0) * 10000)); // 输入单位为万
    const pack = (p, c) => ({
      itemId: p.itemid, name: p.name, count: c,
      hp: p.hp, mp: p.mp, hpPct: p.hpPct, mpPct: p.mpPct, full: p.full,
    });
    return {
      level,
      job: $('#in-job').value,
      mapId: selectedMap ? selectedMap.mapid : null,
      mapName: selectedMap ? selectedMap.mapName : null,
      partyMode,
      gold: goldOf(`#${prefix}-gold`),
      hpPotion: pack(hpItem, count(`#${prefix}-hp-count`)),
      mpPotion: pack(mpItem, count(`#${prefix}-mp-count`)),
      exp: { mode: expMode, input, value: expValueFromInput(level, expMode, input) },
    };
  }

  function buildPayload(start, end, startTime, endTime, activeMs) {
    // 累计表:cumulative[level-1] = 升到该级所需总经验;总进度 = 累计 + 当前级内数值
    const total = (lvl, val) => (DATA.expTable.cumulative[lvl - 1] || 0) + val;
    // 差值字段按服务端约束兜底:负值必被 400 拒,统一 clamp 到合法区间
    const expGained = Math.max(0, end.level === start.level
      ? end.exp.value - start.exp.value
      : total(end.level, end.exp.value) - total(start.level, start.exp.value));
    const goldGained = Math.max(0, end.gold - start.gold);
    const levelsGained = Math.min(100, Math.max(0, end.level - start.level));
    // activeMs 为有效计时(排除暂停时段);服务端约束 1~21600
    const durSec = Math.min(21600, Math.max(1, Math.round((activeMs == null ? endTime - startTime : activeMs) / 1000)));
    const hpUsed = Math.min(1000000, Math.max(0, start.hpPotion.count - end.hpPotion.count));
    const mpUsed = Math.min(1000000, Math.max(0, start.mpPotion.count - end.mpPotion.count));
    const hv = potionSplitValue(start.hpPotion, hpUsed);
    const mv = potionSplitValue(start.mpPotion, mpUsed);
    const potionHpValue = hv.hp + mv.hp; // 回血部分折价(1 点回血 = 1 金币)
    const potionMpValue = hv.mp + mv.mp; // 回蓝部分折价(1 点回蓝 = 2 金币)
    const perHour = (v) => Math.round((v * 3600) / durSec); // 折算到 1 小时标准单位
    return {
      deviceId,
      level: end.level,
      job: start.job,
      mapId: start.mapId,
      mapName: start.mapName,
      partyMode: start.partyMode,
      startTime: new Date(startTime).toISOString(),
      endTime: new Date(endTime).toISOString(),
      durationSeconds: durSec,
      start,
      end,
      delta: {
        gold: goldGained,
        hpPotionUsed: hpUsed,
        mpPotionUsed: mpUsed,
        expGained,
        levelsGained,
      },
      // 收益报告:经验/金币按比例折算到 1 小时(标准单位),药水折价按回血/回蓝分开上报
      profit: {
        durationSeconds: durSec,
        expGained,
        expPerHour: perHour(expGained),
        goldGained,
        goldPerHour: perHour(goldGained),
        potionHpValue,
        potionMpValue,
        potionValue: potionHpValue + potionMpValue, // 药水总折价(服务端可选,一并上报)
      },
    };
  }

  // 药水折价拆分:回血部分 1 点 = 1 金币,回蓝部分 1 点 = 2 金币;全满/百分比恢复无法界定 → 不计算
  function potionSplitValue(p, used) {
    if (!p || used <= 0 || p.full || p.hpPct || p.mpPct) return { hp: 0, mp: 0 };
    return { hp: Math.round(used * p.hp), mp: Math.round(used * p.mp * 2) };
  }

  // ---------- 状态切换 ----------
  function showState(name) {
    closePopup();
    currentState = name;
    document.querySelectorAll('.state').forEach((s) => s.classList.add('hidden'));
    $(`#state-${name}`).classList.remove('hidden');
    // 仅"待开始记录"态允许收缩;计时/结束填写/成功页一律保持展开
    if (name === 'input') armCollapse();
    else {
      clearCollapseTimer();
      if (collapsed) window.mxdApi.setCollapsed(false);
    }
  }

  function flashInvalid(selector) {
    const el = $(selector);
    el.classList.add('err');
    el.focus();
    setTimeout(() => el.classList.remove('err'), 1200);
  }

  // ---------- 计时器 ----------
  function formatHMS(ms) {
    const s = Math.floor(ms / 1000);
    const h = String(Math.floor(s / 3600)).padStart(2, '0');
    const m = String(Math.floor((s % 3600) / 60)).padStart(2, '0');
    const sec = String(s % 60).padStart(2, '0');
    return `${h}:${m}:${sec}`;
  }

  function timerTick() {
    const elapsed = pausedAt ? elapsedBase : elapsedBase + (Date.now() - startTs);
    $('#timer-clock').textContent = formatHMS(elapsed);
  }

  function startTimer() {
    startTs = Date.now();
    elapsedBase = 0;
    pausedAt = 0;
    $('#btn-pause').textContent = '暂停';
    $('#timer-clock').classList.remove('paused');
    timerTick();
    timerInterval = setInterval(timerTick, 200);
  }

  function pauseTimer() {
    if (pausedAt) return;
    elapsedBase += Date.now() - startTs; // 冻结累计值,暂停时段不计入
    pausedAt = Date.now();
    clearInterval(timerInterval);
    timerInterval = null;
    $('#btn-pause').textContent = '继续';
    $('#timer-clock').classList.add('paused');
  }

  function resumeTimer() {
    if (!pausedAt) return;
    startTs = Date.now();
    pausedAt = 0;
    timerTick();
    timerInterval = setInterval(timerTick, 200);
    $('#btn-pause').textContent = '暂停';
    $('#timer-clock').classList.remove('paused');
  }

  // ---------- 事件 ----------
  function bindEvents() {
    // 无标题栏 → 右上角关闭按钮(mousedown 触发)
    $('#btn-close').addEventListener('mousedown', () => window.mxdApi.closeApp());

    // 无标题栏 → 空白处按住拖动窗口(JS 实现,不干扰任何控件的点击)
    let dragState = null;
    document.addEventListener('mousedown', (e) => {
      if (e.button !== 0) return;
      // 收缩成粗线时整条线都可拖动(通常悬停已先还原,此处兜底)
      if (!e.target.closest('.bar') && !document.body.classList.contains('collapsed')) return;
      if (e.target.closest('input, select, button, .seg')) return; // 控件区域不启动拖动
      dragState = { x: e.screenX, y: e.screenY };
      window.mxdApi.dragStart();
    });
    document.addEventListener('mousemove', (e) => {
      if (!dragState) return;
      const dx = e.screenX - dragState.x;
      const dy = e.screenY - dragState.y;
      if (Math.abs(dx) + Math.abs(dy) >= 2) window.mxdApi.dragMove(dx, dy);
    });
    const endDrag = () => {
      if (!dragState) return;
      dragState = null;
      window.mxdApi.dragEnd();
    };
    document.addEventListener('mouseup', endDrag);
    window.addEventListener('blur', endDrag);
    window.addEventListener('blur', () => {
      // 主窗口失焦时 Rust 侧会收起悬浮面板 → 同步本地状态,否则再点输入框面板打不开
      // (时分选择器可聚焦、需键盘输入,失焦属正常,不能在此收起)
      if (popupOpen && popupOpen !== 'time') closePopup();
    });

    // ---------- 吸顶/收缩:进出窗口驱动展开与收缩计时 ----------
    // 从窗口外移入 → 悬停还原(未收缩时 Rust 侧忽略,幂等)
    document.addEventListener('mouseover', (e) => {
      if (e.relatedTarget !== null) return;
      hovered = true;
      clearCollapseTimer();
      window.mxdApi.setCollapsed(false);
    });
    // Alt+Tab 回窗口时 Chromium 可能不重发 mouseover,任何移动/按下都视为悬停
    document.addEventListener('mousemove', () => {
      hovered = true;
      clearCollapseTimer();
      if (collapsed) window.mxdApi.setCollapsed(false);
    });
    document.addEventListener('mousedown', () => {
      hovered = true;
      clearCollapseTimer();
      if (collapsed) window.mxdApi.setCollapsed(false);
    });
    document.addEventListener('mouseout', (e) => {
      if (e.relatedTarget !== null) return;
      hovered = false;
      armCollapse(); // 移出窗口 → 3 秒后收缩
    });
    window.addEventListener('blur', () => {
      hovered = false; // 切回游戏等场景鼠标位置不可知 → 按离开处理
      armCollapse();
    });
    window.mxdApi.onSnapChanged(({ snapped }) => {
      snapOn = snapped;
      if (snapped) armCollapse();
      else { clearCollapseTimer(); setCollapsedClass(false); } // 离开吸顶时 Rust 已强制还原
    });
    window.mxdApi.onCollapsedChanged(({ collapsed: c }) => setCollapsedClass(c));

    // 悬浮面板回传选择
    window.mxdApi.popup.onPick(({ kind, action, hour, minute, second, mapid, key, itemid }) => {
      if (kind === 'map') {
        // 按 mapid 回查:快速输入时行号已错位,当前列表没有再全表兜底
        const m = currentMapList.find((x) => String(x.mapid) === String(mapid))
          || DATA.maps.find((x) => String(x.mapid) === String(mapid));
        if (m) chooseMap(m);
      } else if (kind === 'potion') {
        // 回传 key+itemid 直接定位,不依赖打开面板时的上下文(主窗口失焦会清空上下文,竞态下选不中)
        const p = (POTS[potKindOf(key)] || []).find((x) => x.itemid === itemid);
        if (p) {
          potionSel[key] = p;
          setPotionBtn($(`#${key}-potion-btn`), p);
          const countEl = $(`#${key}-count`);
          if (countEl) { countEl.focus(); countEl.select(); }
          persistSettings();
        }
        closePopup();
      } else if (kind === 'time') {
        closePopup();
        const target = timePickerTarget;
        timePickerTarget = null;
        if (action !== 'ok' || !target) return;
        if (target === '999') {
          checkin999Anchor = timeAnchorOf(hour, minute);
        } else if (target === 'merchant') {
          // 神秘商人:时分秒是"距下次刷新的剩余时长"(0:1:30 = 还剩1分30秒),倒推锚点
          merchantAnchor = Date.now() - (MERCHANT_MS - (hour * 3600 + minute * 60 + (second || 0)) * 1000);
        } else {
          // BOSS:时分秒即死亡时刻(一天中的时刻)
          bossAnchor = bossTimeAnchorOf(hour, minute, second);
        }
        updateTimers();
        persistSettings();
      }
    });

    // 地图组合框(输入于主条,列表悬浮于面板)
    const mapInput = $('#in-map');
    // 用户主动点击输入框(mousedown 只有真实点击才有)→ 清除"刚选择"标志,允许重新搜索
    mapInput.addEventListener('mousedown', () => { mapJustPicked = false; });
    mapInput.addEventListener('focus', () => {
      // 选中地图后面板隐藏,Chromium 会把焦点还给输入框(focus 但无 mousedown)→ 吸收,不重开面板
      if (mapJustPicked) { mapJustPicked = false; return; }
      mapCursor = -1;
      openMapPopup();
    });
    mapInput.addEventListener('input', (e) => {
      // 面板已关闭时的 input = 输入法在选中后才提交的组合文本回写 → 恢复已选地图
      if (popupOpen !== 'map') {
        if (selectedMap) mapInput.value = selectedMap.mapName;
        return;
      }
      if (e.isComposing) return; // 拼音组合中不筛选(列表不会闪成"没有匹配的地图"),提交后自动刷新
      mapCursor = -1;
      renderMapPopup();
    });
    mapInput.addEventListener('compositionend', () => {
      // 输入法提交候选后刷新列表:部分环境下提交不产生最终 input 事件,只靠 input 会漏掉
      if (popupOpen === 'map') renderMapPopup();
    });
    mapInput.addEventListener('blur', () => {
      closePopup();
      // 未选中的搜索词不残留,恢复显示已选地图
      mapInput.value = selectedMap ? selectedMap.mapName : '';
    });
    mapInput.addEventListener('keydown', (e) => {
      // 中文输入法组合中:方向键选候选、回车提交候选,全部交给输入法,不当作列表操作
      if (e.isComposing || e.keyCode === 229) return;
      if (e.key === 'ArrowDown') { e.preventDefault(); moveMapCursor(1); }
      else if (e.key === 'ArrowUp') { e.preventDefault(); moveMapCursor(-1); }
      else if (e.key === 'Enter') { e.preventDefault(); chooseCursorMap(); }
      else if (e.key === 'Escape') { closePopup(); mapInput.blur(); }
    });

    // 药水图标按钮 → 打开悬浮图标格
    document.querySelectorAll('.potion-btn').forEach((btn) => {
      btn.addEventListener('click', () => openPotionPopup(btn));
    });

    // 999打卡 / 神秘商人 / BOSS计时 → 打开时分(秒)选择器(BOSS点任一标签都设置死亡时刻)
    $('#checkin-999').addEventListener('click', () => openTimePicker('999'));
    $('#checkin-merchant').addEventListener('click', () => openTimePicker('merchant'));
    $('#boss-death').addEventListener('click', () => openTimePicker('boss'));
    $('#boss-elapsed').addEventListener('click', () => openTimePicker('boss'));
    // 商人刷新通知:鼠标移到框上 → 停止振动并恢复倒计时
    $('#checkin-merchant').addEventListener('mouseenter', () => {
      merchantRefreshed = false;
      merchantHovered = true;
      updateTimers();
    });
    $('#checkin-merchant').addEventListener('mouseleave', () => { merchantHovered = false; });
    // 点击主条其他位置时收起药水面板
    document.addEventListener('mousedown', (e) => {
      if (popupOpen === 'potion' && !e.target.closest('.potion-btn')) closePopup();
    });

    // EXP 模式切换(两处同步,仅 EXP 的两个 seg)
    document.querySelectorAll('#seg-start, #seg-end').forEach((seg) => {
      seg.addEventListener('click', (e) => {
        const btn = e.target.closest('button[data-mode]');
        if (!btn || btn.disabled) return;
        expMode = btn.dataset.mode;
        refreshExpUI();
        persistSettings();
      });
    });

    // 组队/单人切换
    $('#seg-party').addEventListener('click', (e) => {
      const btn = e.target.closest('button[data-mode]');
      if (!btn) return;
      partyMode = btn.dataset.mode;
      refreshPartyUI();
      persistSettings();
    });

    // 等级/EXP 输入变化 → 刷新模式按钮可用性
    ['in-level', 'in-exp', 'out-level', 'out-exp'].forEach((id) => {
      $(`#${id}`).addEventListener('input', refreshExpUI);
    });

    // 等级/职业修改 → 记忆
    ['in-level', 'out-level'].forEach((id) => $(`#${id}`).addEventListener('change', persistSettings));
    $('#in-job').addEventListener('change', persistSettings);

    // 药水数量:只允许数字;点击默认全选,方便直接改数(延后到默认光标定位之后)
    ['in-hp-count', 'in-mp-count', 'out-hp-count', 'out-mp-count'].forEach((id) => {
      const el = $(`#${id}`);
      el.addEventListener('input', () => { el.value = el.value.replace(/[^\d]/g, ''); });
      el.addEventListener('focus', (e) => setTimeout(() => e.target.select(), 0));
    });

    // EXP:只允许数字与一个小数点;点击默认全选
    ['in-exp', 'out-exp'].forEach((id) => {
      const el = $(`#${id}`);
      el.addEventListener('input', () => {
        const v = el.value.replace(/[^\d.]/g, '');
        const i = v.indexOf('.');
        el.value = i >= 0 ? v.slice(0, i + 1) + v.slice(i + 1).replace(/\./g, '') : v;
      });
      el.addEventListener('focus', (e) => setTimeout(() => e.target.select(), 0));
    });

    // 等级/金币/地图:点击默认全选(地图全选 → 打字直接替换旧地图名,不会拼出"勇士部落南港东部"这种搜不到的词)
    ['in-level', 'out-level', 'in-gold', 'out-gold', 'in-map'].forEach((id) => {
      $(`#${id}`).addEventListener('focus', (e) => setTimeout(() => e.target.select(), 0));
    });

    // 开始记录
    $('#btn-start').addEventListener('click', () => {
      const level = clampLevel($('#in-level').value);
      if (!$('#in-level').value || level < 1 || level > 200) { flashInvalid('#in-level'); return; }
      if (!selectedMap) { flashInvalid('#in-map'); return; }
      startSnapshot = readSnapshot('in');
      $('#timer-clock').textContent = '00:00:00';
      showState('timer');
      startTimer();
    });

    // 停止记录 → 结束 BAR 预填
    $('#btn-stop').addEventListener('click', () => {
      const endTs = Date.now();
      if (timerInterval) clearInterval(timerInterval);
      timerInterval = null;
      stopActiveMs = pausedAt ? elapsedBase : elapsedBase + (endTs - startTs); // 暂停中停止也算暂停时长
      $('#out-level').value = startSnapshot.level;
      $('#out-gold').value = startSnapshot.gold / 10000; // 回显单位为万
      potionSel['out-hp'] = findPot('hp', startSnapshot.hpPotion.itemId);
      setPotionBtn($('#out-hp-potion-btn'), potionSel['out-hp']);
      $('#out-hp-count').value = startSnapshot.hpPotion.count;
      potionSel['out-mp'] = findPot('mp', startSnapshot.mpPotion.itemId);
      setPotionBtn($('#out-mp-potion-btn'), potionSel['out-mp']);
      $('#out-mp-count').value = startSnapshot.mpPotion.count;
      $('#out-exp').value = startSnapshot.exp.input;
      expMode = startSnapshot.exp.mode;
      refreshExpUI();
      showState('end');
      stopTs = endTs;
      $('#out-gold').focus();
      $('#out-gold').select();
    });

    // 暂停/继续:暂停时段不计入有效计时
    $('#btn-pause').addEventListener('click', () => {
      if (pausedAt) resumeTimer();
      else pauseTimer();
    });

    // 取消计时 → 本次计时作废,回到输入页(输入内容原样保留)
    $('#btn-cancel-timer').addEventListener('click', () => {
      if (timerInterval) clearInterval(timerInterval);
      timerInterval = null;
      startSnapshot = null;
      elapsedBase = 0;
      pausedAt = 0;
      showState('input');
    });

    // 提交数据
    $('#btn-submit').addEventListener('click', async () => {
      if (submitting) return;
      const level = clampLevel($('#out-level').value);
      if (!$('#out-level').value || level < 1 || level > 200) { flashInvalid('#out-level'); return; }
      const end = readSnapshot('out');
      const payload = buildPayload(startSnapshot, end, startTs, stopTs, stopActiveMs);
      submitting = true;
      $('#btn-submit').disabled = true;
      $('#btn-submit').textContent = '提交中…';
      try {
        const r = await window.mxdApi.submitRecord(payload);
        if (!startSnapshot) return; // 提交期间点了取消 → 已回输入页,不再跳成功页
        if (r && r.ok) {
          // 用服务端返参拼出本设备分享链接,替换成功页文案与按钮目标
          if (r.shareUrl) {
            const link = $('#done-link');
            link.textContent = r.shareUrl;
            link.href = r.shareUrl;
          }
          showState('done');
          $('#btn-submit').textContent = '提交数据'; // 复位,下次提交不再显示"提交中…"
        } else {
          console.error('提交失败:', r && `${r.status} ${r.error}`);
          $('#btn-submit').textContent = '提交失败,重试';
        }
      } catch (e) {
        console.error('提交失败:', e);
        $('#btn-submit').textContent = '提交失败,重试';
      } finally {
        submitting = false;
        $('#btn-submit').disabled = false;
      }
    });

    // 取消本次记录 → 放弃此次数据,回到输入页
    $('#btn-cancel-end').addEventListener('click', () => {
      startSnapshot = null;
      $('#btn-submit').textContent = '提交数据'; // 复位按钮文案(可能处于"提交中…/提交失败,重试")
      showState('input');
    });

    // 成功页:立即前往 / 点击链接 → 打开本设备分享链接
    const openShare = () => window.mxdApi.openSite($('#done-link').href || null);
    $('#btn-visit').addEventListener('click', openShare);
    $('#done-link').addEventListener('click', (e) => { e.preventDefault(); openShare(); });

    // 重新开始 → 回到初始输入页
    $('#btn-restart').addEventListener('click', () => {
      startSnapshot = null;
      expMode = 'value';
      partyMode = 'solo';
      refreshPartyUI();
      // 等级接续上次结束时的等级(记忆),金币/药水数量等每次重新数
      $('#in-level').value = clampLevel($('#out-level').value);
      $('#in-gold').value = 0;
      $('#in-hp-count').value = 0;
      $('#in-mp-count').value = 0;
      $('#in-exp').value = 0;
      // 地图与药水选择保留(已记忆),返回不用重新选
      $('#in-map').value = selectedMap ? selectedMap.mapName : '';
      $('#out-level').value = 1;
      $('#out-gold').value = 0;
      $('#out-hp-count').value = 0;
      $('#out-mp-count').value = 0;
      $('#out-exp').value = 0;
      refreshExpUI();
      persistSettings(); // 记住接续等级,重启程序也不丢
      showState('input');
      $('#in-level').focus();
    });
  }

  init().catch((e) => {
    console.error(e);
    // Tauri 的 IPC 拒绝是字符串,取 e.message || e 避免显示 undefined
    document.body.innerHTML = '<div style="padding:20px;color:#e05a5a">数据加载失败:' + (e && e.message || e) + '</div>';
  });
})();
