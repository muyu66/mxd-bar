// CDP 驱动测试:连接 WebView2 调试端口(默认 9222,可用 CDP_PORT 覆盖),端到端验证主条 + 悬浮面板 + 计时器全流程
// 用法:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9222" ./target/debug/mxd-bar.exe 后运行本脚本
import { execSync } from 'node:child_process';

const PORT = process.env.CDP_PORT || 9222;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const targets = async () => (await (await fetch(`http://127.0.0.1:${PORT}/json`)).json()).filter((t) => t.type === 'page');

// 面板关闭验证用屏幕哈希:WebView2 的 document.hidden 对 SW_SHOWNOACTIVATE 显示的窗口不可靠,
// 直接比对面板区域的屏幕像素,面板隐藏后区域内容必然变化
const grabHash = (x, y, w = 900, h = 300) =>
  execSync(
    `powershell -NoProfile -ExecutionPolicy Bypass -File "${new URL('./screen-hash.ps1', import.meta.url).pathname.replace(/^\//, '')}" -X ${x} -Y ${y} -W ${w} -H ${h}`,
    { encoding: 'utf8' }
  ).trim();

function connect(wsUrl) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(wsUrl);
    let id = 0;
    const pending = new Map();
    ws.onopen = () =>
      resolve({
        send: (method, params) =>
          new Promise((res, rej) => {
            const mid = ++id;
            pending.set(mid, { res, rej });
            ws.send(JSON.stringify({ id: mid, method, params }));
          }),
        close: () => ws.close(),
      });
    ws.onerror = () => reject(new Error('ws error'));
    ws.onmessage = (e) => {
      const m = JSON.parse(e.data);
      if (m.id && pending.has(m.id)) {
        const { res, rej } = pending.get(m.id);
        pending.delete(m.id);
        m.error ? rej(new Error(m.error.message)) : res(m.result);
      }
    };
  });
}

const evalIn = async (c, expr) => {
  const r = await c.send('Runtime.evaluate', { expression: expr, returnByValue: true, awaitPromise: true });
  if (r.exceptionDetails) throw new Error('JS异常: ' + (r.exceptionDetails.exception?.description || r.exceptionDetails.text));
  return r.result.value;
};

const results = [];
const check = (name, ok, detail = '') => {
  results.push(`${ok ? '✅' : '❌'} ${name}${detail ? ` — ${detail}` : ''}`);
  console.log(results[results.length - 1]);
};

// ---- 1. 主条初始状态 ----
const mainPage = (await targets()).find((p) => p.title === '冒险岛怀旧服经验记录器');
if (!mainPage) { console.error('❌ 找不到主条页面'); process.exit(1); }
const main = await connect(mainPage.webSocketDebuggerUrl);
const tgt = async () => (await targets()).find((p) => p.url.includes('popup.html'));
const winPos = JSON.parse(await evalIn(main, 'JSON.stringify({x: screenX, y: screenY})'));

check('主条桥接与初始渲染', await evalIn(main, `typeof window.mxdApi === 'object' && !document.querySelector('#state-input').classList.contains('hidden')`));
check('药水图标(data URL)', (await evalIn(main, `document.getElementById('in-hp-potion-btn').innerHTML`)).includes('data:image'));

// ---- 2. 药水面板:打开/渲染/回传 ----
await evalIn(main, `document.getElementById('in-hp-potion-btn').click()`);
await sleep(1200);
let popup = await tgt();
check('药水面板创建并渲染', !!popup && (await (async () => {
  const p = await connect(popup.webSocketDebuggerUrl);
  const n = await evalIn(p, `document.querySelectorAll('.popup-potion').length`);
  p.close();
  return n;
})()) > 0);
if (popup) {
  const p = await connect(popup.webSocketDebuggerUrl);
  const expected = await evalIn(p, `document.querySelectorAll('.popup-potion')[0].title`);
  const hOpen = grabHash(winPos.x, winPos.y + 38);
  await evalIn(p, `document.querySelectorAll('.popup-potion')[0].dispatchEvent(new MouseEvent('mousedown', { bubbles: true }))`);
  await sleep(800);
  const after = await evalIn(main, `document.getElementById('in-hp-potion-btn').title`);
  check('药水选择回传(事件链路)', after === expected, after.slice(0, 30));
  await sleep(500);
  const hClosed = grabHash(winPos.x, winPos.y + 38);
  check('药水面板已关闭(屏幕像素)', hOpen !== hClosed, `${hOpen} → ${hClosed}`);
  p.close();
}

// ---- 3. 地图面板 ----
await evalIn(main, `document.getElementById('in-map').focus(); document.getElementById('in-map').dispatchEvent(new Event('focus'))`);
// 等上一轮 potion 面板关闭流程(计数框抢焦点)落定
await sleep(1200);
popup = await tgt();
if (popup) {
  const p = await connect(popup.webSocketDebuggerUrl);
  const rows = await evalIn(p, `document.querySelectorAll('.popup-opt').length`);
  check('地图面板渲染列表', rows > 0, `${rows} 条`);
  // 输入过滤词
  await evalIn(main, `(() => { const i = document.getElementById('in-map'); i.value = '勇士'; i.dispatchEvent(new Event('input')); })()`);
  await sleep(800);
  const filtered = await evalIn(p, `document.querySelectorAll('.popup-opt').length`);
  check('地图搜索过滤', filtered > 0 && filtered < 60, `${filtered} 条`);
  // 选第一条 → 回传
  const hOpen = grabHash(winPos.x, winPos.y + 38);
  await evalIn(p, `document.querySelectorAll('.popup-opt')[0].dispatchEvent(new MouseEvent('mousedown', { bubbles: true }))`);
  await sleep(800);
  const mapVal = await evalIn(main, `document.getElementById('in-map').value`);
  check('地图选择回传', !!mapVal, mapVal);
  await sleep(500);
  const hClosed = grabHash(winPos.x, winPos.y + 38);
  check('地图选择后面板已关闭(屏幕像素)', hOpen !== hClosed, `${hOpen} → ${hClosed}`);
  p.close();
} else {
  check('地图面板创建', false);
}

// ---- 4. 时间选择器(focusable 路径) ----
await evalIn(main, `document.getElementById('checkin-999').click()`);
await sleep(1200);
popup = await tgt();
if (popup) {
  const p = await connect(popup.webSocketDebuggerUrl);
  const hasTime = await evalIn(p, `!!document.querySelector('.popup-time')`);
  check('时间选择器渲染', hasTime);
  // 点"确定"按钮 → 主条锚点更新
  await evalIn(p, `document.querySelector('.popup-time-btns button').dispatchEvent(new MouseEvent('mousedown', { bubbles: true }))`);
  await sleep(800);
  const t = await evalIn(main, `document.getElementById('checkin-999').textContent`);
  check('打卡锚点已设置(不再是"待打卡")', t !== '待打卡', t);
  p.close();
} else {
  check('时间选择器创建', false);
}

// ---- 5. 计时器全流程:开始 → 暂停 → 继续 → 停止 → 取消 ----
const startLevel = await evalIn(main, `document.getElementById('in-level').value`);
await evalIn(main, `document.getElementById('btn-start').click()`);
await sleep(400);
const timerOn = await evalIn(main, `!document.querySelector('#state-timer').classList.contains('hidden')`);
check('开始记录 → 计时器态', timerOn);
const t0 = await evalIn(main, `document.getElementById('timer-clock').textContent`);
await evalIn(main, `document.getElementById('btn-pause').click()`);
await sleep(1500);
const t1 = await evalIn(main, `document.getElementById('timer-clock').textContent`);
await sleep(1000);
const t2 = await evalIn(main, `document.getElementById('timer-clock').textContent`);
check('暂停后时钟冻结', t1 === t2, `${t1}`);
const pauseLabel = await evalIn(main, `document.getElementById('btn-pause').textContent`);
check('按钮切换为"继续"', pauseLabel === '继续', pauseLabel);
await evalIn(main, `document.getElementById('btn-pause').click()`);
await sleep(1200);
const t3 = await evalIn(main, `document.getElementById('timer-clock').textContent`);
check('继续后时钟恢复走时', t3 !== t2, `${t3}`);
await evalIn(main, `document.getElementById('btn-stop').click()`);
await sleep(400);
const endOn = await evalIn(main, `!document.querySelector('#state-end').classList.contains('hidden')`);
check('停止 → 结束 BAR 预填', endOn);
const outLevel = await evalIn(main, `document.getElementById('out-level').value`);
check('结束等级预填', String(outLevel) === String(startLevel), outLevel);
await evalIn(main, `document.getElementById('btn-cancel-end').click()`);
await sleep(400);
check('结束页取消 → 回输入页', await evalIn(main, `!document.querySelector('#state-input').classList.contains('hidden')`));

// ---- 6. 计时器取消(作废) ----
await evalIn(main, `document.getElementById('btn-start').click()`);
await sleep(300);
await evalIn(main, `document.getElementById('btn-cancel-timer').click()`);
await sleep(400);
check('计时中取消 → 回输入页', await evalIn(main, `!document.querySelector('#state-input').classList.contains('hidden')`));

// ---- 7. 持久化验证(等级记忆 + 药水记忆) ----
const saved = await evalIn(main, `window.mxdApi.getSettings()`);
check('设置已持久化(含等级/药水/打卡锚点)', saved && saved.level && saved.potions && saved.checkin999, JSON.stringify(saved).slice(0, 90));

main.close();
console.log('\n==== ' + results.filter((r) => r.startsWith('✅')).length + '/' + results.length + ' 项通过 ====');
process.exit(results.some((r) => r.startsWith('❌')) ? 1 : 0);
