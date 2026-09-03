// 收缩粗线验证:吸顶 → 收缩 → 输出真实矩形(有无隐形边框)→ 保持收缩态退出(供截屏)
// 用法: node scripts/verify-line.mjs(需先关闭正在运行的 mxd-bar)
import { execFileSync, spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const PORT = process.env.CDP_PORT || 9223;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const ps = (code) =>
  execFileSync('powershell', ['-NoProfile', '-Command', '-'], {
    input: `Add-Type -TypeDefinition 'using System; using System.Runtime.InteropServices; public class W{[DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr a, int x, int y, int cx, int cy, uint f); [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r); [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, uint a, out RECT r, uint s); [DllImport("dwmapi.dll", EntryPoint = "DwmGetWindowAttribute")] public static extern int DwmGetWindowAttributeI(IntPtr h, uint a, out int r, uint s); public struct RECT { public int L; public int T; public int R; public int B; }}'\n` + code,
    encoding: 'utf8',
  }).trim();
const hwndCode = (pid) => `
$h = (Get-Process -Id ${pid} -ErrorAction Stop).MainWindowHandle
if ($h -eq [IntPtr]::Zero) { Write-Output 'NOTFOUND'; exit 1 }
`;
const moveWin = (pid, x, y) =>
  ps(hwndCode(pid) + `[W]::SetWindowPos($h, [IntPtr]::Zero, ${x}, ${y}, 0, 0, 0x0005) | Out-Null
Write-Output ok`);
// 两种矩形:GetWindowRect(含隐形边框)与 ExtendedFrameBounds(可见区域);另读回圆角属性(33)
const rects = (pid) =>
  ps(hwndCode(pid) + `$wr = New-Object W+RECT; $ef = New-Object W+RECT; $cp = 0
[W]::GetWindowRect($h, [ref]$wr) | Out-Null
[W]::DwmGetWindowAttribute($h, 9, [ref]$ef, 16) | Out-Null
[W]::DwmGetWindowAttributeI($h, 33, [ref]$cp, 4) | Out-Null
Write-Output "WR:$($wr.L),$($wr.T),$($wr.R),$($wr.B)"
Write-Output "EF:$($ef.L),$($ef.T),$($ef.R),$($ef.B)"
Write-Output "CORNER:$cp (2=ROUND)"`);

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
    ws.onopen = () => resolve({
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

const SETTINGS = path.join(process.env.APPDATA, 'mxd-exp-recorder', 'settings.ini');
let iniBackup = null;
try { iniBackup = fs.readFileSync(SETTINGS, 'utf8'); } catch (e) { /* 无旧设置 */ }
fs.writeFileSync(SETTINGS, `[window]\nx=100\ny=300\n[player]\nlevel=1\n`);

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
  if (!main) throw new Error('找不到主条页面:可能已有旧实例在运行,请先关闭后重试');
  await sleep(500);

  moveWin(child.pid, 100, 0); // 拖到顶部 → 吸顶
  await sleep(800);
  await evalIn(main, `window.mxdApi.setCollapsed(true)`);
  await sleep(1000);
  console.log(rects(child.pid));
  const geom = await evalIn(main, `({ h: window.outerHeight, ih: window.innerHeight, y: window.screenY })`);
  console.log('CSS px:', JSON.stringify(geom));
  // 截取窗口四周 30px 屏幕区域,落盘供像素分析
  const shot = ps(hwndCode(child.pid) + `Add-Type -AssemblyName System.Drawing
$ef = New-Object W+RECT
[W]::DwmGetWindowAttribute($h, 9, [ref]$ef, 16) | Out-Null
$pad = 30
$x = [Math]::Max(0, $ef.L - $pad); $y = [Math]::Max(0, $ef.T - $pad)
$w = ($ef.R - $ef.L) + 2 * $pad; $h = ($ef.B - $ef.T) + 2 * $pad
$bmp = New-Object System.Drawing.Bitmap $w, $h
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($x, $y, 0, 0, $bmp.Size)
$bmp.Save("C:\\Web\\mxd-exp\\snap-shot2.png", [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()
Write-Output "shot:$w x $h"`);
  console.log(shot);
  main.close(); // 关闭 CDP 连接,让 node 能正常退出
  await sleep(300);
} catch (e) {
  console.error('❌', e.message || e);
  try { main && main.close(); } catch (e2) { /* 忽略 */ }
} finally {
  try { child.kill(); } catch (e2) { /* 忽略 */ }
  try { if (iniBackup) fs.writeFileSync(SETTINGS, iniBackup); } catch (e2) { /* 忽略 */ }
  process.exit(0);
}
