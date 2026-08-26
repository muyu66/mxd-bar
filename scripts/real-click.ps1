# 系统级真实鼠标点击:SetCursorPos + mouse_event,走完整 Windows 输入管线(含 WM_MOUSEACTIVATE/命中测试)
param(
  [int]$X,
  [int]$Y
)
Add-Type @'
using System;
using System.Runtime.InteropServices;
public class Mouse {
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
}
'@
[Mouse]::SetCursorPos($X, $Y) | Out-Null
Start-Sleep -Milliseconds 120
[Mouse]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)  # LEFTDOWN
Start-Sleep -Milliseconds 60
[Mouse]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)  # LEFTUP
