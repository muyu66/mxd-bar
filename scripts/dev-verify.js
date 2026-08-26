// 开发用 DOM 结构验证:程序化断言各状态布局/数值/文案,输出文本报告。
// 用法: npx electron scripts/dev-verify.js
const { app, BrowserWindow, ipcMain } = require('electron');
const { execFileSync } = require('node:child_process');
const { randomUUID } = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');

let mainWindow = null;
let deviceId;
try {
  const out = execFileSync('reg', ['query', 'HKLM\\SOFTWARE\\Microsoft\\Cryptography', '/v', 'MachineGuid'], { encoding: 'utf8' });
  deviceId = out.match(/MachineGuid\s+REG_SZ\s+([0-9a-fA-F-]+)/)[1].toLowerCase();
} catch (e) { deviceId = randomUUID(); }

function loadData() {
  const dir = path.join(__dirname, '..', 'data');
  const read = (f) => JSON.parse(fs.readFileSync(path.join(dir, f), 'utf-8'));
  return {
    expTable: read('exp-table.json'), maps: read('maps.json'),
    potions: read('potions.json'), jobs: read('jobs.json'),
    potionIconDir: path.join(dir, 'potion-icons'),
  };
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let pass = 0, fail = 0;
const check = (name, ok, extra = '') => {
  if (ok) { pass++; console.log(`  ✓ ${name}`); }
  else { fail++; console.log(`  ✗ ${name} ${extra}`); }
};

// 悬浮面板 IPC mock:记录渲染负载/关闭次数,把 pick 转发回主窗
let popupRenderPayload = null;
let popupCloseCount = 0;
let closeClicked = false;
let savedPatch = null;
let submittedPayload = null;
let openedUrl = null;

app.whenReady().then(async () => {
  ipcMain.handle('get-device-id', () => deviceId);
  ipcMain.handle('get-data', () => loadData());
  ipcMain.handle('submit-record', (_e, payload) => {
    submittedPayload = payload;
    // 模拟新版服务端:返回本条记录 id(服务端生成,与 deviceId 无关)
    return { ok: true, report: payload, id: 'mj8v3x2a1b9c', shareUrl: 'http://127.0.0.1:3001/exp.html?id=mj8v3x2a1b9c' };
  });
  ipcMain.handle('open-site', (_e, url) => { openedUrl = url; });
  // 模拟"上次关闭时的设置" → 验证启动恢复
  // checkin999: 25 小时前打卡 → 恢复后应显示待打卡;merchant: 5 秒前刷新 → 恢复后约 05:59:55
  ipcMain.handle('get-settings', () => ({
    level: 60, job: '牧师', potions: { 'in-hp': 2000001 }, expMode: 'percent',
    map: { mapid: 100000000, mapName: '射手村', scene: 'town', street: '金银岛' },
    checkin999: Date.now() - 25 * 3600 * 1000,
    merchant: Date.now() - 5000,
  }));
  ipcMain.handle('save-settings', (_e, patch) => { savedPatch = patch; return true; });
  ipcMain.on('popup-show', () => {});
  ipcMain.on('popup-render', (_e, p) => { popupRenderPayload = p; });
  ipcMain.on('popup-close', () => { popupCloseCount++; });
  ipcMain.on('popup-pick', (_e, data) => mainWindow.webContents.send('popup-picked', data));
  ipcMain.on('close-app', () => { closeClicked = true; });

  mainWindow = new BrowserWindow({
    width: 1400, height: 36, frame: false, show: false,
    webPreferences: {
      preload: path.join(__dirname, '..', 'src', 'preload.js'),
      contextIsolation: true, nodeIntegration: false, sandbox: false,
    },
  });
  await mainWindow.loadFile(path.join(__dirname, '..', 'src', 'renderer', 'index.html'));
  mainWindow.show();
  const wc = mainWindow.webContents;
  const js = (code) => wc.executeJavaScript(code, true);
  await sleep(600);

  console.log('— 状态1 输入 BAR —');
  const r1 = await js(`(() => {
    const rects = ['#in-level','#in-job','#in-map','#in-gold','#in-hp-potion-btn','#in-mp-potion-btn','#in-exp','#btn-start','#checkin-999','#checkin-merchant']
      .map(s => document.querySelector(s).getBoundingClientRect());
    return {
      orderOk: rects.every((r, i) => i === 0 || r.left >= rects[i-1].left),
      noOverflow: document.documentElement.scrollWidth <= document.documentElement.clientWidth,
      fitsHeight: document.documentElement.scrollHeight <= document.documentElement.clientHeight,
      hasClose: !!document.querySelector('#btn-close'),
      jobs: document.querySelector('#in-job').options.length,
      hpFirst: document.querySelector('#in-hp-potion-btn').title,
      mpFirst: document.querySelector('#in-mp-potion-btn').title,
      levelVal: document.querySelector('#in-level').value,
      jobVal: document.querySelector('#in-job').value,
      mapVal: document.querySelector('#in-map').value,
      hpIcon: (() => {
        const img = document.querySelector('#in-hp-potion-btn img');
        return !!img && /potion-icons[/\\\\]2000001\\.png/.test(img.src);
      })(),
      goldW: Math.round(document.querySelector('#in-gold').getBoundingClientRect().width),
      goldUnitInside: (() => {
        const i = document.querySelector('#in-gold').getBoundingClientRect();
        const u = document.querySelector('#state-input .unit').getBoundingClientRect();
        return u.left >= i.left && u.right <= i.right;
      })(),
      expW: Math.round(document.querySelector('#in-exp').getBoundingClientRect().width),
      potionCountW: Math.round(document.querySelector('#in-hp-count').getBoundingClientRect().width),
      checkin999: document.querySelector('#checkin-999').textContent,
      checkin999Waiting: document.querySelector('#checkin-999').classList.contains('waiting'),
      merchant: document.querySelector('#checkin-merchant').textContent,
      merchantIdle: document.querySelector('#checkin-merchant').classList.contains('idle'),
      centerOk: (() => {
        const first = document.querySelector('#state-input .field').getBoundingClientRect();
        const bar = document.querySelector('#state-input .bar');
        const last = bar.children[bar.children.length - 1].getBoundingClientRect();
        return Math.abs(first.left - (document.documentElement.clientWidth - last.right)) < 6;
      })(),
      spinHidden: [...document.styleSheets].some((ss) => {
        try {
          return [...ss.cssRules].some((r) => r.selectorText
            && r.selectorText.includes('::-webkit-inner-spin-button')
            && r.style.getPropertyValue('-webkit-appearance') === 'none');
        } catch (e) { return false; }
      }),
      expSeg: [...document.querySelectorAll('#seg-start button')].map(b => b.textContent + ':' + b.classList.contains('active')),
    };
  })()`);
  check('无标题栏关闭按钮存在', r1.hasClose);
  check('36px 高度内无纵向溢出', r1.fitsHeight);
  check('字段从左到右排列', r1.orderOk);
  check('无横向溢出', r1.noOverflow, `scrollWidth=${await js('document.documentElement.scrollWidth')} clientWidth=${await js('document.documentElement.clientWidth')}`);
  check('职业下拉 12 个选项', r1.jobs === 12, `实际 ${r1.jobs}`);
  check('启动恢复等级 60', r1.levelVal === '60', `实际 "${r1.levelVal}"`);
  check('启动恢复职业 牧师', r1.jobVal === '牧师', `实际 "${r1.jobVal}"`);
  check('启动恢复地图 射手村', r1.mapVal === '射手村', `实际 "${r1.mapVal}"`);
  check('启动恢复HP药水 橙色药水', r1.hpFirst === '橙色药水 (HP+150)', `实际 "${r1.hpFirst}"`);
  check('MP 药水默认为 蓝色药水 (MP+100)', r1.mpFirst === '蓝色药水 (MP+100)', `实际 "${r1.mpFirst}"`);
  check('药水图标来自抓取图片', r1.hpIcon);
  check('数字输入隐藏增减按钮', r1.spinHidden);
  check('金币宽度 84px', r1.goldW === 84, `实际 ${r1.goldW}`);
  check('金币单位"万"嵌在输入框内(不占页面空间)', r1.goldUnitInside);
  check('EXP 数值框宽度 112px', r1.expW === 112, `实际 ${r1.expW}`);
  check('药水数量宽度 60px', r1.potionCountW === 60, `实际 ${r1.potionCountW}`);
  check('字段整体居中', r1.centerOk);
  check('EXP 恢复为百分比模式', r1.expSeg[0] === '%:true', JSON.stringify(r1.expSeg));
  check('999打卡:超24小时恢复为待打卡(红)', r1.checkin999 === '待打卡' && r1.checkin999Waiting, `实际 "${r1.checkin999}"`);
  check('神秘商人:恢复后继续倒计时', /^\d{2}:\d{2}:\d{2}$/.test(r1.merchant) && !r1.merchantIdle, `实际 "${r1.merchant}"`);

  console.log('— 设置记忆 —');
  await js(`(() => {
    const setChange = (id, v) => { const el = document.querySelector(id); el.value = v; el.dispatchEvent(new Event('change')); };
    setChange('#in-level', 55);
    setChange('#in-job', '火毒法师');
  })()`);
  await sleep(200);
  check('等级修改被记忆', savedPatch && savedPatch.level === '55', JSON.stringify(savedPatch));
  check('职业修改被记忆', savedPatch && savedPatch.job === '火毒法师', JSON.stringify(savedPatch));

  console.log('— 关闭按钮 —');
  await js(`document.querySelector('#btn-close').dispatchEvent(new MouseEvent('mousedown', { bubbles: true }))`);
  await sleep(100);
  check('关闭按钮触发关闭请求', closeClicked);

  console.log('— 地图悬浮面板 —');
  await js(`(() => {
    const set = (id, v) => { const el = document.querySelector(id); el.value = v; el.dispatchEvent(new Event('input')); };
    set('#in-level', 45);
    document.querySelector('#in-map').focus();
    set('#in-map', '射手');
  })()`);
  await sleep(300);
  const r2 = popupRenderPayload;
  check('搜索"射手"过滤出地图且只显示名字', r2 && r2.kind === 'map' && r2.rows.length > 0
    && r2.rows.every((row) => (row.name.includes('射手') || (row.street || '').includes('射手')) && !/\d{9}/.test(row.name)),
    JSON.stringify(r2 && r2.rows.slice(0, 3)));
  await js(`document.querySelector('#in-map').dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown' }))`);
  await sleep(100);
  await js(`document.querySelector('#in-map').dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown' }))`);
  await sleep(100);
  check('方向键移动光标高亮', popupRenderPayload.cursor === 1, `cursor=${popupRenderPayload && popupRenderPayload.cursor}`);
  await js(`document.querySelector('#in-map').dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowUp' }))`);
  await sleep(100);
  check('方向键可回退光标', popupRenderPayload.cursor === 0, `cursor=${popupRenderPayload && popupRenderPayload.cursor}`);
  mainWindow.webContents.send('popup-picked', { kind: 'map', index: 0 });
  await sleep(300);
  const r2b = await js(`(async () => ({
    value: document.querySelector('#in-map').value,
    focus: await new Promise((res) => {
      const el = document.querySelector('#in-hp-count');
      el.focus();
      setTimeout(() => res({ active: document.activeElement === el, val: el.value, start: el.selectionStart, end: el.selectionEnd }), 50);
    }),
  }))()`);
  check('选中后输入框只显示地图名(无ID)', r2b.value === '射手村', `实际 "${r2b.value}"`);
  check('地图选择被记忆', savedPatch && savedPatch.map && savedPatch.map.mapName === '射手村', JSON.stringify(savedPatch && savedPatch.map));

  // 全站地图:金银岛之外的地图也能搜到,且带所属区域
  await js(`(() => {
    const el = document.querySelector('#in-map');
    el.focus();
    el.value = '龙族打猎场';
    el.dispatchEvent(new Event('input'));
  })()`);
  await sleep(300);
  const rD = popupRenderPayload;
  const dragonRow = rD && rD.rows && rD.rows.find((row) => row.name === '龙族打猎场');
  check('全站地图:可搜到"龙族打猎场"', rD && rD.kind === 'map' && !!dragonRow, JSON.stringify(rD && rD.rows && rD.rows.slice(0, 3)));
  check('地图行附带所属区域(迷宫)', dragonRow && dragonRow.street === '迷宫', JSON.stringify(dragonRow));
  await js(`document.querySelector('#in-map').blur()`);
  check('选中后悬浮面板关闭', popupCloseCount > 0, `popupCloseCount=${popupCloseCount}`);
  check('药水数量点击默认全选', r2b.focus.active && r2b.focus.start === 0 && r2b.focus.end === String(r2b.focus.val).length, JSON.stringify(r2b.focus));

  console.log('— 药水悬浮面板 —');
  await js(`document.querySelector('#in-hp-potion-btn').click()`);
  await sleep(300);
  const r2c = popupRenderPayload;
  check('药水面板渲染图标格(41 种 HP,已去重)', r2c && r2c.kind === 'potion' && r2c.items.length === 41
    && r2c.items.every((it) => /^#/.test(it.color)), `items=${r2c && r2c.items.length}`);
  mainWindow.webContents.send('popup-picked', { kind: 'potion', index: 2 });
  await sleep(300);
  const r2d = await js(`({
    title: document.querySelector('#in-hp-potion-btn').title,
    countFocused: document.activeElement === document.querySelector('#in-hp-count'),
  })`);
  check('选中第3个HP药水(白色)', r2d.title === '白色药水 (HP+300)', `实际 "${r2d.title}"`);
  check('选完药水焦点跳到数量并全选', r2d.countFocused);
  check('药水选择被记忆', savedPatch && savedPatch.potions && savedPatch.potions['in-hp'] === 2000002, JSON.stringify(savedPatch));

  console.log('— 999打卡 / 神秘商人 —');
  await js(`document.querySelector('#checkin-999').click()`);
  await sleep(300);
  const rt = popupRenderPayload;
  check('点击999弹出时分选择器', rt && rt.kind === 'time' && rt.hour >= 0 && rt.hour <= 23 && rt.minute >= 0 && rt.minute <= 59, JSON.stringify(rt));
  const nowD = new Date();
  mainWindow.webContents.send('popup-picked', { kind: 'time', action: 'ok', hour: nowD.getHours(), minute: nowD.getMinutes() });
  await sleep(300);
  const r999 = await js(`(() => ({
    text: document.querySelector('#checkin-999').textContent,
    waiting: document.querySelector('#checkin-999').classList.contains('waiting'),
  }))()`);
  check('打卡后从23:59:59开始绿色倒计时', r999.text === '23:59:59' && !r999.waiting, r999.text);
  check('打卡时间被记忆(时间戳)', typeof savedPatch.checkin999 === 'number', JSON.stringify(savedPatch && savedPatch.checkin999));

  await js(`document.querySelector('#checkin-merchant').click()`);
  await sleep(300);
  check('点击商人弹出时分秒选择器且默认05:59:59', popupRenderPayload && popupRenderPayload.kind === 'time'
    && popupRenderPayload.hour === 5 && popupRenderPayload.minute === 59 && popupRenderPayload.second === 59
    && popupRenderPayload.withSeconds === true && popupRenderPayload.maxHour === 5,
    JSON.stringify(popupRenderPayload && { hour: popupRenderPayload.hour, minute: popupRenderPayload.minute, second: popupRenderPayload.second, maxHour: popupRenderPayload.maxHour }));
  mainWindow.webContents.send('popup-picked', { kind: 'time', action: 'ok', hour: 5, minute: 59, second: 59 });
  await sleep(300);
  const rM = await js(`(() => ({
    text: document.querySelector('#checkin-merchant').textContent,
    idle: document.querySelector('#checkin-merchant').classList.contains('idle'),
  }))()`);
  check('商人输入5:59:59 → 约6小时内倒计时(05:59:5x 起)', /^05:59:5\d$/.test(rM.text) && !rM.idle, rM.text);
  check('商人锚点被记忆(时间戳)', typeof savedPatch.merchant === 'number', JSON.stringify(savedPatch && savedPatch.merchant));

  // 回归:商人的时分秒是"剩余时长"而非一天中的时刻 —— 0:1 = 还剩约1分钟刷新
  await js(`document.querySelector('#checkin-merchant').click()`);
  await sleep(300);
  mainWindow.webContents.send('popup-picked', { kind: 'time', action: 'ok', hour: 0, minute: 1, second: 0 });
  await sleep(300);
  const rM1 = await js(`document.querySelector('#checkin-merchant').textContent`);
  check('商人输入0:1:0 → 剩余约1分钟倒计时', /^00:0[01]:\d{2}$/.test(rM1), rM1);

  // 回归:秒字段参与计算 —— 0:1:5 = 还剩1分5秒
  await js(`document.querySelector('#checkin-merchant').click()`);
  await sleep(300);
  mainWindow.webContents.send('popup-picked', { kind: 'time', action: 'ok', hour: 0, minute: 1, second: 5 });
  await sleep(300);
  const rM2 = await js(`document.querySelector('#checkin-merchant').textContent`);
  check('商人输入0:1:5 → 剩余约1分5秒倒计时', /^00:01:0[45]$/.test(rM2), rM2);

  // 倒计时结束(周期翻转)→ 振动并显示"已刷新";悬停 → 停止振动恢复倒计时
  await js(`document.querySelector('#checkin-merchant').click()`);
  await sleep(300);
  mainWindow.webContents.send('popup-picked', { kind: 'time', action: 'ok', hour: 0, minute: 0, second: 1 });
  await sleep(1600); // 越过 0 点触发周期翻转
  const rN = await js(`(() => ({
    text: document.querySelector('#checkin-merchant').textContent,
    refreshed: document.querySelector('#checkin-merchant').classList.contains('refreshed'),
  }))()`);
  check('倒计时结束 → 显示"已刷新"并振动', rN.text === '已刷新' && rN.refreshed, JSON.stringify(rN));
  await js(`document.querySelector('#checkin-merchant').dispatchEvent(new MouseEvent('mouseenter'))`);
  await sleep(100);
  const rH = await js(`(() => ({
    text: document.querySelector('#checkin-merchant').textContent,
    refreshed: document.querySelector('#checkin-merchant').classList.contains('refreshed'),
  }))()`);
  check('悬停商人框 → 停止振动恢复倒计时', /^\d{2}:\d{2}:\d{2}$/.test(rH.text) && !rH.refreshed, JSON.stringify(rH));
  await js(`document.querySelector('#checkin-merchant').dispatchEvent(new MouseEvent('mouseleave'))`);

  console.log('— EXP 双模式 —');
  const r3 = await js(`(async () => {
    const set = (id, v) => { const el = document.querySelector(id); el.value = v; el.dispatchEvent(new Event('input')); };
    document.querySelector('#seg-start button[data-mode="percent"]').click();
    const pctActive = document.querySelector('#seg-start button[data-mode="percent"]').classList.contains('active');
    const noHint = !document.querySelector('#exp-hint-start');
    set('#in-level', 200);
    const pctDisabled = document.querySelector('#seg-start button[data-mode="percent"]').disabled;
    set('#in-level', 45);
    document.querySelector('#seg-start button[data-mode="value"]').click();
    set('#in-exp', 1234567);
    const selectAll = await new Promise((res) => {
      const el = document.querySelector('#in-exp');
      el.focus();
      setTimeout(() => res(el.selectionStart === 0 && el.selectionEnd === el.value.length), 50);
    });
    return { pctActive, noHint, pctDisabled, selectAll };
  })()`);
  check('百分比模式可切换', r3.pctActive);
  check('EXP 无换算提示行', r3.noHint);
  check('200 级时百分比按钮禁用', r3.pctDisabled);
  check('EXP 点击默认全选', r3.selectAll);
  check('EXP 模式修改被记忆', savedPatch && savedPatch.expMode === 'value', JSON.stringify(savedPatch));

  console.log('— 组队/单人 —');
  const r3p = await js(`(() => {
    const seg = document.querySelector('#seg-party');
    const soloDefault = seg.querySelector('button[data-mode="solo"]').classList.contains('active');
    seg.querySelector('button[data-mode="party"]').click();
    const partyActive = seg.querySelector('button[data-mode="party"]').classList.contains('active')
      && !seg.querySelector('button[data-mode="solo"]').classList.contains('active');
    return { soloDefault, partyActive };
  })()`);
  await sleep(200);
  check('模式选项卡默认单人', r3p.soloDefault);
  check('点击切换为组队(高亮互斥)', r3p.partyActive);
  check('组队/单人选择被记忆', savedPatch && savedPatch.partyMode === 'party', JSON.stringify(savedPatch && savedPatch.partyMode));

  console.log('— 状态2 计时器 —');
  await js(`(() => {
    const set = (id, v) => { const el = document.querySelector(id); el.value = v; el.dispatchEvent(new Event('input')); };
    set('#in-gold', 1000); set('#in-hp-count', 50); set('#in-mp-count', 30); set('#in-exp', 1234567);
    document.querySelector('#btn-start').click();
  })()`);
  await sleep(2200);
  const r4 = await js(`(() => ({
    visible: !document.querySelector('#state-timer').classList.contains('hidden'),
    clock: document.querySelector('#timer-clock').textContent,
    noInfo: !document.querySelector('.timer-info'),
    stopGap: (() => {
      const c = document.querySelector('#timer-clock').getBoundingClientRect();
      const b = document.querySelector('#btn-stop').getBoundingClientRect();
      return b.left - c.right;
    })(),
  }))()`);
  check('计时器态可见', r4.visible);
  check('计时器格式 HH:MM:SS 且已走字', /^\d{2}:\d{2}:\d{2}$/.test(r4.clock) && r4.clock !== '00:00:00', r4.clock);
  check('计时器不显示等级/地图信息', r4.noInfo);
  check('停止按钮紧邻倒计时(不贴右缘)', r4.stopGap >= 0 && r4.stopGap <= 40, `gap=${r4.stopGap}`);

  console.log('— 状态3 结束 BAR 预填 —');
  await js(`document.querySelector('#btn-stop').click()`);
  await sleep(300);
  const r5 = await js(`(() => ({
    visible: !document.querySelector('#state-end').classList.contains('hidden'),
    level: document.querySelector('#out-level').value,
    gold: document.querySelector('#out-gold').value,
    hpCount: document.querySelector('#out-hp-count').value,
    mpCount: document.querySelector('#out-mp-count').value,
    hpPotion: document.querySelector('#out-hp-potion-btn').title,
    exp: document.querySelector('#out-exp').value,
    hasJobField: !!document.querySelector('#state-end .f-job'),
    hasMapField: !!document.querySelector('#state-end .f-map'),
    submitText: document.querySelector('#btn-submit').textContent,
  }))()`);
  check('结束态可见且无职业/地图字段', r5.visible && !r5.hasJobField && !r5.hasMapField);
  check('等级/金币/药水/EXP 预填一致', r5.level === '45' && r5.gold === '1000' && r5.hpCount === '50' && r5.mpCount === '30' && r5.exp === '1234567', JSON.stringify(r5));
  check('药水图标预填为白色药水', r5.hpPotion === '白色药水 (HP+300)', `实际 "${r5.hpPotion}"`);
  check('提交按钮文案', r5.submitText === '提交数据');

  // 收益报告:修改结束值再提交 → 断言 profit 计算(经验/金币折算每小时,药水折价)
  await js(`(() => {
    const set = (id, v) => { const el = document.querySelector(id); el.value = v; el.dispatchEvent(new Event('input')); };
    set('#out-exp', 1259567);   // +25000 经验
    set('#out-gold', 1012.5);   // +12.5 万 = 125000 金币
    set('#out-hp-count', 40);   // 白色药水(HP+300)用 10 个
    set('#out-mp-count', 25);   // 蓝色药水(MP+100)用 5 个
  })()`);

  console.log('— 状态4 成功动画 —');
  await js(`document.querySelector('#btn-submit').click()`);
  await sleep(400);
  const r6 = await js(`(() => ({
    visible: !document.querySelector('#state-done').classList.contains('hidden'),
    msg: document.querySelector('.done-msg').textContent,
    visit: document.querySelector('#btn-visit').textContent,
    restart: document.querySelector('#btn-restart').textContent,
  }))()`);
  check('成功态可见', r6.visible);
  check('提示文案含分享链接(exp.html?id=记录id)', r6.msg.includes('/exp.html?id=mj8v3x2a1b9c'), r6.msg);
  check('两个按钮:立即前往/返回', r6.visit === '立即前往' && r6.restart === '返回');
  const r6b = await js(`document.querySelector('#btn-submit').textContent`);
  check('提交成功后按钮复位为"提交数据"', r6b === '提交数据', `实际 "${r6b}"`);
  await js(`document.querySelector('#btn-visit').click()`);
  await sleep(100);
  check('立即前往打开分享链接', openedUrl === 'http://127.0.0.1:3001/exp.html?id=mj8v3x2a1b9c', String(openedUrl));

  console.log('— 收益报告 —');
  const P = submittedPayload;
  check('收益报告写入上报JSON', !!P && !!P.profit
    && Number.isFinite(P.profit.expPerHour) && Number.isFinite(P.profit.goldPerHour), JSON.stringify(P && P.profit));
  check('经验收益:增量 25000 且每小时折算一致', !!P && P.profit.expGained === 25000
    && P.profit.expPerHour === Math.round(25000 * 3600 / P.profit.durationSeconds), JSON.stringify(P && P.profit));
  check('金币收益:增量 125000 且每小时折算一致', !!P && P.profit.goldGained === 125000
    && P.profit.goldPerHour === Math.round(125000 * 3600 / P.profit.durationSeconds), JSON.stringify(P && P.profit));
  check('药水折价分开上报:HP 3000 / MP 1000 (1回血=1金币,1回蓝=2金币)', !!P
    && P.profit.potionHpValue === 3000 && P.profit.potionMpValue === 1000,
    JSON.stringify(P && P.profit && { potionHpValue: P.profit.potionHpValue, potionMpValue: P.profit.potionMpValue }));
  check('delta 与收益报告一致(金币/药水用量)', !!P && P.delta.gold === 125000 && P.delta.hpPotionUsed === 10 && P.delta.mpPotionUsed === 5, JSON.stringify(P && P.delta));
  check('上报JSON含组队/单人模式', !!P && P.partyMode === 'party', JSON.stringify(P && P.partyMode));

  console.log('— 重新开始 —');
  await js(`document.querySelector('#btn-restart').click()`);
  await sleep(300);
  const r7 = await js(`(() => ({
    visible: !document.querySelector('#state-input').classList.contains('hidden'),
    level: document.querySelector('#in-level').value,
    gold: document.querySelector('#in-gold').value,
    map: document.querySelector('#in-map').value,
    exp: document.querySelector('#in-exp').value,
    hpPotion: document.querySelector('#in-hp-potion-btn').title,
    partySolo: document.querySelector('#seg-party button[data-mode="solo"]').classList.contains('active'),
  }))()`);
  check('回到输入页且字段已清空', r7.visible && r7.level === '1' && r7.gold === '0' && r7.exp === '0', JSON.stringify(r7));
  check('返回后地图保留(射手村,无需重新输入)', r7.map === '射手村', `实际 "${r7.map}"`);
  check('药水重置为默认红色药水', r7.hpPotion === '红色药水 (HP+50)', `实际 "${r7.hpPotion}"`);
  check('重新开始后模式回到单人', r7.partySolo);

  console.log(`\n结果: ${pass} 通过 / ${fail} 失败`);
  console.log('deviceId:', deviceId);
  app.exit(fail ? 1 : 0);
});
