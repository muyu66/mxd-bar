// 复现"选任何地图都显示勇士部落东部":真实鼠标点输入框 → 真实键盘输入 → 点非第一行 → 比对
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

// 1. 真实鼠标点击地图输入框
const rect = JSON.parse(await evalIn(m, 'JSON.stringify((() => { const r = document.getElementById("in-map").getBoundingClientRect(); return {x: r.x, y: r.y, w: r.width, h: r.height}; })())'));
await clickAt(m, Math.round(rect.x + rect.w / 2), Math.round(rect.y + rect.h / 2));
await sleep(1500);

// 2. 真实键盘输入过滤词(先清空再输入,模拟用户操作)
await evalIn(m, `document.getElementById('in-map').select()`);
await m.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Backspace', code: 'Backspace', windowsVirtualKeyCode: 8 });
await m.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Backspace', code: 'Backspace', windowsVirtualKeyCode: 8 });
await sleep(300);
await m.send('Input.insertText', { text: '勇士' });
await sleep(1200);

// 3. 读面板行(纯名字,不含区域后缀)
const popupPage = (await targets()).find((p) => p.url.includes('popup.html'));
if (!popupPage) { console.error('❌ 面板未出现'); process.exit(1); }
const p = await connect(popupPage.webSocketDebuggerUrl);
const rows = JSON.parse(await evalIn(p, 'JSON.stringify([...document.querySelectorAll(".popup-opt")].map((e) => e.childNodes[0].textContent))'));
console.log('面板行(' + rows.length + '):', rows.slice(0, 6).join(' | '));

// 4a. 真实鼠标点第 3 行(索引 2)
if (rows.length >= 3) {
  const rowRect = JSON.parse(await evalIn(p, 'JSON.stringify((() => { const r = document.querySelectorAll(".popup-opt")[2].getBoundingClientRect(); return {x: r.x, y: r.y, w: r.width, h: r.height}; })())'));
  await clickAt(p, Math.round(rowRect.x + rowRect.w / 2), Math.round(rowRect.y + rowRect.h / 2));
  await sleep(1500);
  const mapVal = await evalIn(m, 'document.getElementById("in-map").value');
  console.log('鼠标点第 3 行:', rows[2], '→ in-map 显示:', mapVal, rows[2] === mapVal ? '✅' : '❌');
} else {
  console.log('行数不足 3,跳过鼠标点击');
}

// 4b. 重新打开面板,回车选择(不移动光标 → 应选第一个匹配项)
await evalIn(m, `document.getElementById('in-map').value = ''`);
await clickAt(m, Math.round(rect.x + rect.w / 2), Math.round(rect.y + rect.h / 2));
await sleep(1200);
await m.send('Input.insertText', { text: '勇士' });
await sleep(1000);
await m.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13 });
await m.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13 });
await sleep(1200);
const enterVal = await evalIn(m, 'document.getElementById("in-map").value');
console.log('回车选择 → in-map 显示:', enterVal, '应为第一个匹配项:', rows[0], enterVal === rows[0] ? '✅' : '❌');
p.close();
m.close();
