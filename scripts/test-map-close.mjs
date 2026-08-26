// 地图面板关闭验证:打开面板 → 屏幕哈希 → 选择地图 → 屏幕哈希,比对面板是否真的从屏幕消失
import { execSync } from 'node:child_process';

const PORT = process.env.CDP_PORT || 9222;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const targets = async () =>
  (await (await fetch(`http://127.0.0.1:${PORT}/json`)).json()).filter((t) => t.type === 'page');

function connect(wsUrl) {
  return new Promise((resolve) => {
    const ws = new WebSocket(wsUrl);
    let id = 0;
    const pending = new Map();
    ws.onopen = () =>
      resolve({
        send: (m, p) =>
          new Promise((res) => {
            const i = ++id;
            pending.set(i, res);
            ws.send(JSON.stringify({ id: i, method: m, params: p }));
          }),
        close: () => ws.close(),
      });
    ws.onmessage = (e) => {
      const m = JSON.parse(e.data);
      if (m.id && pending.has(m.id)) {
        pending.get(m.id)(m.result);
        pending.delete(m.id);
      }
    };
  });
}
const evalIn = async (c, expr) =>
  (await c.send('Runtime.evaluate', { expression: expr, returnByValue: true, awaitPromise: true })).result?.value;

const grabHash = () =>
  execSync(`powershell -NoProfile -ExecutionPolicy Bypass -File C:/Web/mxd-exp/scripts/screen-hash.ps1`, {
    encoding: 'utf8',
  }).trim();

const mainPage = (await targets()).find((p) => p.title === '冒险岛怀旧服经验记录器');
const m = await connect(mainPage.webSocketDebuggerUrl);

// 打开地图面板
await evalIn(
  m,
  `(() => { const i = document.getElementById('in-map'); i.value=''; i.focus(); i.dispatchEvent(new Event('focus')); })()`
);
await sleep(2000);
const h1 = grabHash();
console.log('面板打开时:', h1);

// 选择第一条地图
const popupPage = (await targets()).find((p) => p.url.includes('popup.html'));
const p = await connect(popupPage.webSocketDebuggerUrl);
await evalIn(
  p,
  `document.querySelectorAll('.popup-opt')[0].dispatchEvent(new MouseEvent('mousedown', { bubbles: true }))`
);
await sleep(1500);
const h2 = grabHash();
console.log('选择之后:  ', h2);
console.log(h1 === h2 ? '❌ 屏幕无变化,面板仍显示' : '✅ 屏幕发生变化,面板已隐藏');
p.close();
m.close();
