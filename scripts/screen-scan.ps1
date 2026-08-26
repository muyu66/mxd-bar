# 全屏扫描:输出非全黑的行范围,用于定位窗口实际位置
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
$b = New-Object System.Drawing.Bitmap([System.Windows.Forms.Screen]::PrimaryScreen.Bounds.Width, [System.Windows.Forms.Screen]::PrimaryScreen.Bounds.Height)
$g = [System.Drawing.Graphics]::FromImage($b)
$g.CopyFromScreen(0, 0, 0, 0, $b.Size)
$W = $b.Width; $H = $b.Height
Write-Output ("screen " + $W + "x" + $H)
# 每 16px 采样,统计非黑色块
$rows = @{}
$cols = @{}
for ($y = 0; $y -lt $H; $y += 16) {
  for ($x = 0; $x -lt $W; $x += 16) {
    $c = $b.GetPixel($x, $y)
    if ($c.R + $c.G + $c.B -gt 30) {
      $rows[[int]([math]::Floor($y / 16))] = $true
      $cols[[int]([math]::Floor($x / 16))] = $true
    }
  }
}
function spans($map) {
  $ks = @($map.Keys | Sort-Object)
  if ($ks.Count -eq 0) { return "(全黑)" }
  $out = @(); $s = $ks[0]; $p = $ks[0]
  foreach ($k in $ks) { if ($k - $p -gt 1) { $out += ("$s-$p"); $s = $k }; $p = $k }
  $out += "$s-$p"
  return ($out -join ",")
}
Write-Output ("非黑行(每格16px): " + (spans $rows))
Write-Output ("非黑列(每格16px): " + (spans $cols))
$g.Dispose(); $b.Dispose()
