# 截取屏幕区域并输出像素哈希(用于判断悬浮面板是否真的从屏幕上消失)
param(
  [int]$X = 280,
  [int]$Y = 150,
  [int]$W = 900,
  [int]$H = 300
)
Add-Type -AssemblyName System.Drawing
$b = New-Object System.Drawing.Bitmap($W, $H)
$g = [System.Drawing.Graphics]::FromImage($b)
$g.CopyFromScreen($X, $Y, 0, 0, $b.Size)
$hash = 0
for ($y = 0; $y -lt $H; $y += 12) {
  for ($x = 0; $x -lt $W; $x += 12) {
    $c = $b.GetPixel($x, $y)
    $hash = ($hash * 31 + $c.R + $c.G + $c.B) -band 0x7FFFFFFF
  }
}
Write-Output ("hash=" + $hash)
$g.Dispose()
$b.Dispose()
