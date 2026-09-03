// CDP 驱动吸顶功能测试:真实移动窗口 → 吸顶判定 → 收缩成粗线 → 999/商人闪亮提醒 → 还原 → 拖离退出吸顶 → 3 秒自动收缩
// 自启动 debug 版 mxd-bar(带 CDP 端口),测试完杀掉进程。
// 窗口移动走内联 PowerShell(-Command -,不落盘 .ps1、不绕执行策略),按 PID 取主窗口句柄;
// 窗口尺寸/位置断言走页面 window.outerHeight/screenY(CSS px,不受 DWM 阴影影响)。
// 用法: node scripts/test-snap.mjs(需先关闭正在运行的 mxd-bar)
import { execFileSync, spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const PORT = process.env.CDP_PORT || 9223;
const MERCHANT_MS = 6 * 3600 * 1000;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ---------- 窗口控制(内联 PowerShell,纯 ASCII) ----------
const ps = (code) =>
  execFileSync('powershell', ['-NoProfile', '-Command', '-'], {
    input: `Add-Type -TypeDefinition 'using System; using System.Runtime.InteropServices; public class W{[DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr a, int x, int y, int cx, int cy, uint f); [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y); [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, uint a, out RECT r, uint s); public struct RECT { public int L; public int T; public int R; public int B; }}'\n` + code,
    encoding: 'utf8',
  }).trim();
const hwndCode = (pid) => `
$h = (Get-Process -Id ${pid} -ErrorAction Stop).MainWindowHandle
if ($h -eq [IntPtr]::Zero) { Write-Output 'NOTFOUND'; exit 1 }
`;
const moveWin = (pid, x, y) =>
  ps(hwndCode(pid) + `[W]::SetWindowPos($h, [IntPtr]::Zero, ${x}, ${y}, 0, 0, 0x0005) | Out-Null
Write-Output ok`);
const moveCursor = (x, y) => ps(`[W]::SetCursorPos(${x}, ${y}) | Out-Null; Write-Output ok`);
// 真实窗口矩形(物理 px,不含 DWM 阴影)
const winRectPhys = (pid) =>
  ps(hwndCode(pid) + `$r = New-Object W+RECT
[W]::DwmGetWindowAttribute($h, 9, [ref]$r, 16) | Out-Null
Write-Output "$($r.L) $($r.T) $($r.R) $($r.B)"`);

// ---------- CDP ----------
const targets = async () => {
  try {
    return (await (await fetch(`http://127.0.0.1:${PORT}/json`)).json()).filter((t) => t.type === 'page');
  } catch (e) { return []; }
};
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

// ---------- 预写 settings.ini:窗口 (100,300)、999 无锚点(待打卡)、商人 8 秒后周期翻转 ----------
const SETTINGS = path.join(process.env.APPDATA, 'mxd-exp-recorder', 'settings.ini');
let iniBackup = null;
let iniBackedUp = false;
try {
  iniBackup = fs.readFileSync(SETTINGS, 'utf8');
  iniBackedUp = true;
} catch (e) { /* 无旧设置 */ }
fs.writeFileSync(
  SETTINGS,
  `[window]\nx=100\ny=300\n[player]\nlevel=1\n[checkin]\nmerchant=${Date.now() - MERCHANT_MS + 8000}\n`
);

const child = spawn('target/debug/mxd-bar.exe', [], {
  cwd: fileURLToPath(new URL('../src-tauri', import.meta.url)),
  env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${PORT}` },
  stdio: 'ignore',
});

let main = null;
try {
  for (let i = 0; i < 60 && !main; i++) {
    await sleep(250);
    const page = (await targets()).find((p) => p.title === '冒险岛怀旧服经验记录器');
    if (page) main = await connect(page.webSocketDebuggerUrl);
  }
  if (!main) throw new Error('找不到主条页面:可能已有旧实例在运行(单实例互斥),请先关闭后重试');
  await sleep(500);
  check('页面桥接正常', await evalIn(main, `typeof window.mxdApi.getSnapState === 'function'`));

  // 鼠标移出窗口区(吸顶收缩前 Rust 侧会复核鼠标位置)
  moveCursor(500, 400);
  // 窗口几何:CSS px(outerHeight/innerHeight)+ 真实物理矩形(不含 DWM 阴影)
  const geom = () => evalIn(main, `({ x: window.screenX, y: window.screenY, h: window.outerHeight, ih: window.innerHeight })`);
  const phys = () => winRectPhys(child.pid);

  // ---- 1. 初始:窗口在 (100,300),非吸顶 ----
  const s0 = await evalIn(main, `window.mxdApi.getSnapState()`);
  check('初始非吸顶', s0.snapped === false, JSON.stringify(s0));

  // ---- 2. 拖到顶部 → 吸顶开启并贴顶 ----
  moveWin(child.pid, 100, 0);
  await sleep(800);
  const s1 = await evalIn(main, `window.mxdApi.getSnapState()`);
  check('拖到顶部 → 吸顶开启', s1.snapped === true, JSON.stringify(s1));
  const g1 = await geom();
  check('窗口贴合屏幕顶部', g1.y <= 40, `y=${g1.y}`);
  check('展开高度 36 CSS px', g1.h === 36, `${JSON.stringify(g1)} phys=${await phys()}`);

  // ---- 3. 收缩成粗线 ----
  await evalIn(main, `window.mxdApi.setCollapsed(true)`);
  await sleep(800);
  const g2 = await geom();
  check('收缩成一根粗线(约 6 CSS px)', g2.h >= 4 && g2.h <= 8, `${JSON.stringify(g2)} phys=${await phys()}`);
  check('收缩后仍贴顶', g2.y <= 40, `y=${g2.y}`);
  check('收缩样式生效(内容隐藏、粗线显示)', await evalIn(main,
    `document.body.classList.contains('collapsed') && getComputedStyle(document.querySelector('#snap-line')).display !== 'none'
     && getComputedStyle(document.querySelector('#app')).display === 'none'`));

  // ---- 4. 粗线提醒:999 待打卡 → 红闪;随后商人周期翻转 → 红金交替闪 ----
  check('999 待打卡 → 粗线红闪', await evalIn(main, `document.body.classList.contains('alert-999')`));
  check('商人未翻转前不闪金', await evalIn(main, `!document.body.classList.contains('alert-merchant')`));
  await sleep(4500); // 越过 8 秒商人翻转点(期间保持收缩)
  check('商人已刷新 → 粗线红金交替闪', await evalIn(main, `document.body.classList.contains('alert-both')`));

  // ---- 5. 悬停还原 ----
  await evalIn(main, `window.mxdApi.setCollapsed(false)`);
  await sleep(800);
  const g3 = await geom();
  check('悬停 → 还原高度', g3.h === 36, `${JSON.stringify(g3)} phys=${await phys()}`);
  check('还原后样式移除', await evalIn(main, `!document.body.classList.contains('collapsed')`));

  // ---- 6. 拖离顶部 → 退出吸顶 ----
  moveWin(child.pid, 100, 300);
  await sleep(800);
  const s2 = await evalIn(main, `window.mxdApi.getSnapState()`);
  check('拖离顶部 → 吸顶关闭', s2.snapped === false, JSON.stringify(s2));
  const g4 = await geom();
  check('拖离后保持展开高度', g4.h === 36, `${JSON.stringify(g4)} phys=${await phys()}`);

  // ---- 7. 重新吸顶后无悬停 3 秒 → 自动收缩(渲染层计时路径) ----
  moveWin(child.pid, 100, 0);
  await sleep(800);
  moveCursor(500, 400); // 确保鼠标不在窗口内
  await sleep(3800); // 3 秒收缩计时 + 余量
  const g5 = await geom();
  check('无悬停 3 秒 → 自动收缩', g5.h <= 8, `${g5.h}px`);
  await evalIn(main, `window.mxdApi.setCollapsed(false)`); // 复位,便于手动复检
} catch (e) {
  console.error('❌ 测试异常:', e.message || e);
  results.push('❌ 测试异常终止');
} finally {
  try { main && main.close(); } catch (e) { /* 忽略 */ }
  try { child.kill(); } catch (e) { /* 忽略 */ }
  if (iniBackedUp) fs.writeFileSync(SETTINGS, iniBackup); // 还原用户设置
}

console.log('\n==== ' + results.filter((r) => r.startsWith('✅')).length + '/' + results.length + ' 项通过 ====');
process.exit(results.some((r) => r.startsWith('❌')) ? 1 : 0);
