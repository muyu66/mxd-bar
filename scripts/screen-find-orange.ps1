# 全屏找冒险岛橙(#f5a623 附近)像素,定位主条位置
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
$b = New-Object System.Drawing.Bitmap([System.Windows.Forms.Screen]::PrimaryScreen.Bounds.Width, [System.Windows.Forms.Screen]::PrimaryScreen.Bounds.Height)
$g = [System.Drawing.Graphics]::FromImage($b)
$g.CopyFromScreen(0, 0, 0, 0, $b.Size)
$found = @()
for ($y = 0; $y -lt $b.Height; $y += 4) {
  for ($x = 0; $x -lt $b.Width; $x += 4) {
    $c = $b.GetPixel($x, $y)
    if ($c.R -gt 200 -and $c.G -gt 120 -and $c.G -lt 210 -and $c.B -lt 110) {
      $found += ("($x,$y)")
    }
  }
}
if ($found.Count -eq 0) { Write-Output "no-orange" } else {
  Write-Output ("orange count=" + $found.Count + " at " + (($found | Select-Object -First 12) -join " "))
}
$g.Dispose(); $b.Dispose()
