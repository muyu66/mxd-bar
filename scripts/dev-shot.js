// 开发用自动走查脚本:加载真实 preload + 真实数据,依次截图 5 个状态。
// 用法: npx electron scripts/dev-shot.js  (截图输出到 shots/)
const { app, BrowserWindow, ipcMain, shell } = require('electron');
const { execFileSync } = require('node:child_process');
const { randomUUID } = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');

const SHOTS = path.join(__dirname, '..', 'shots');
fs.mkdirSync(SHOTS, { recursive: true });
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

app.whenReady().then(async () => {
  ipcMain.handle('get-device-id', () => deviceId);
  ipcMain.handle('get-data', () => loadData());
  ipcMain.handle('get-settings', () => ({}));
  ipcMain.handle('save-settings', () => true);
  ipcMain.handle('submit-record', (_e, payload) => {
    console.log('[mock submit] durationSeconds:', payload.durationSeconds, '| expGained:', payload.delta.expGained);
    return { ok: true, report: payload, id: 'mj8v3x2a1b9c', shareUrl: 'http://127.0.0.1:3001/exp.html?id=mj8v3x2a1b9c' };
  });
  ipcMain.handle('open-site', (_e, url) => { console.log('[open-site]', url); });
  // 悬浮面板 mock:pick 直接转发回主窗
  ipcMain.on('popup-show', () => {});
  ipcMain.on('popup-render', () => {});
  ipcMain.on('popup-close', () => {});
  ipcMain.on('popup-pick', (_e, data) => mainWindow.webContents.send('popup-picked', data));
  ipcMain.on('close-app', () => {});

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
  const shot = async (name) => {
    const img = await wc.capturePage();
    fs.writeFileSync(path.join(SHOTS, `${name}.png`), img.toPNG());
    console.log('shot:', name);
  };
  await sleep(800);

  // 1. 输入页:填值 + 搜索并选择地图(悬浮面板用 pick 模拟)
  await js(`(() => {
    const set = (id, v) => { const el = document.querySelector(id); el.value = v; el.dispatchEvent(new Event('input')); };
    set('#in-level', 45); set('#in-gold', 123456); set('#in-hp-count', 50); set('#in-mp-count', 30);
    set('#in-exp', 1234567);
    document.querySelector('#in-map').focus();
    set('#in-map', '射手');
    window.mxdApi.popup.pick({ kind: 'map', index: 0 });
  })()`);
  await sleep(400);
  await shot('1-input-ready');

  // 2. 开始记录 → 计时器
  await js(`document.querySelector('#btn-start').click()`);
  await sleep(1300);
  await shot('2-timer');

  // 3. 停止 → 结束页(预填)
  await js(`document.querySelector('#btn-stop').click()`);
  await sleep(400);
  await shot('3-end-prefilled');

  // 结束值修改:EXP 改为 2345678,金币 124000
  await js(`(() => {
    const set = (id, v) => { const el = document.querySelector(id); el.value = v; el.dispatchEvent(new Event('input')); };
    set('#out-exp', 2345678); set('#out-gold', 124000); set('#out-hp-count', 38); set('#out-mp-count', 24);
  })()`);
  await sleep(200);

  // 4. 提交 → 成功动画
  await js(`document.querySelector('#btn-submit').click()`);
  await sleep(800);
  await shot('4-done');

  // 5. 重新开始 → 回到输入页
  await js(`document.querySelector('#btn-restart').click()`);
  await sleep(400);
  await shot('5-restart');

  console.log('deviceId used:', deviceId);
  app.quit();
});
