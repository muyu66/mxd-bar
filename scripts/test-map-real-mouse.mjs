// 真实鼠标复现:通过 CDP Input.dispatchMouseEvent 走 WebView2 真实输入管线,模拟用户点地图输入框 → 选地图
const PORT = process.env.CDP_PORT || 9333;
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

const clickAt = async (c, x, y) => {
  await c.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x, y });
  await c.send('Input.dispatchMouseEvent', { type: 'mousePressed', x, y, button: 'left', clickCount: 1 });
  await sleep(60);
  await c.send('Input.dispatchMouseEvent', { type: 'mouseReleased', x, y, button: 'left', clickCount: 1 });
};

let mainPage = null;
for (let i = 0; i < 20 && !mainPage; i++) {
  mainPage = (await targets()).find((p) => p.title === '冒险岛怀旧服经验记录器');
  if (!mainPage) await sleep(1000);
}
if (!mainPage) { console.error('主窗口页面未出现'); process.exit(1); }
const m = await connect(mainPage.webSocketDebuggerUrl);

const pos = JSON.parse(await evalIn(m, 'JSON.stringify({x: screenX, y: screenY})'));
console.log('主窗口位置:', JSON.stringify(pos));

// 地图输入框中心(视口坐标)
const rect = JSON.parse(await evalIn(m, 'JSON.stringify((() => { const r = document.getElementById("in-map").getBoundingClientRect(); return {x: r.x, y: r.y, w: r.width, h: r.height}; })())'));
const inMapCenter = { x: Math.round(rect.x + rect.w / 2), y: Math.round(rect.y + rect.h / 2) };
console.log('地图输入框中心:', JSON.stringify(inMapCenter));

// 真实鼠标点击地图输入框
await clickAt(m, inMapCenter.x, inMapCenter.y);
await sleep(1800);

let popupPage = (await targets()).find((p) => p.url.includes('popup.html'));
console.log('点击后 popup target:', popupPage ? popupPage.url : '❌ 未出现');
if (!popupPage) process.exit(1);
const p = await connect(popupPage.webSocketDebuggerUrl);

const popupPos = JSON.parse(await evalIn(p, 'JSON.stringify({x: screenX, y: screenY, w: outerWidth, h: outerHeight})'));
console.log('面板屏幕位置:', JSON.stringify(popupPos));
const rows = await evalIn(p, 'document.querySelectorAll(".popup-opt").length');
console.log('面板行数:', rows);

if (rows > 0) {
  const rowRect = JSON.parse(await evalIn(p, 'JSON.stringify((() => { const r = document.querySelectorAll(".popup-opt")[0].getBoundingClientRect(); return {x: r.x, y: r.y, w: r.width, h: r.height}; })())'));
  console.log('第一行视口位置:', JSON.stringify(rowRect));
  // 真实鼠标点击第一行
  await clickAt(p, Math.round(rowRect.x + rowRect.w / 2), Math.round(rowRect.y + rowRect.h / 2));
  await sleep(1500);
  const mapVal = await evalIn(m, 'document.getElementById("in-map").value');
  console.log('点击后 in-map 值:', JSON.stringify(mapVal));
  console.log(mapVal ? '✅ 选择成功' : '❌ 选择失败');
  const hidden = await evalIn(p, 'document.visibilityState');
  console.log('面板 visibilityState:', hidden);
}
p.close();
m.close();
