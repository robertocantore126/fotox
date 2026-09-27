# Fotox — real OS input and pixels for the running app, in page (client) pixels.
#
#   powershell -NoProfile -File ui/tools/win.ps1 pixels 100,200 640,300
#   powershell -NoProfile -File ui/tools/win.ps1 click 640,300 [shift|ctrl|alt]
#   powershell -NoProfile -File ui/tools/win.ps1 drag 600,300 700,380 [steps]
#   powershell -NoProfile -File ui/tools/win.ps1 move 640,300
#
# Pointer input over the viewport goes from the OS straight to the engine
# (winit), never through the embedded browser, so DevTools cannot press on
# the canvas: this moves the real cursor. The document canvas is drawn by
# wgpu under the UI, so a DevTools screenshot does not show it either:
# `pixels` reads what is really on screen ("r,g,b" per point). The window is
# brought to the front first.
param([string]$Command, [Parameter(ValueFromRemainingArguments = $true)][string[]]$Rest)

Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class FxWin {
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L; public int T; public int R; public int B; }
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint f, UIntPtr e);
}
"@
[void][FxWin]::SetProcessDPIAware()
$proc = Get-Process fotox -ErrorAction Stop | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
if (-not $proc) { throw "no Fotox window" }
$h = $proc.MainWindowHandle
[void][FxWin]::SetForegroundWindow($h)
Start-Sleep -Milliseconds 120
$origin = New-Object FxWin+POINT
[void][FxWin]::ClientToScreen($h, [ref]$origin)

function To-Screen([string]$p) {
  $xy = $p.Split(",") | ForEach-Object { [int][Math]::Round([double]$_) }
  return @(($origin.X + $xy[0]), ($origin.Y + $xy[1]))
}
$MODS = @{ shift = 0x10; ctrl = 0x11; alt = 0x12 }
function Hold([string]$mod, [bool]$down) {
  if ($mod -and $MODS.ContainsKey($mod)) { [FxWin]::keybd_event([byte]$MODS[$mod], 0, $(if ($down) { 0 } else { 2 }), [UIntPtr]::Zero) }
}
function Move-To([int[]]$s) { [void][FxWin]::SetCursorPos($s[0], $s[1]); Start-Sleep -Milliseconds 15 }
$LEFTDOWN = 0x0002; $LEFTUP = 0x0004

switch ($Command) {
  "pixels" {
    $rect = New-Object FxWin+RECT
    [void][FxWin]::GetClientRect($h, [ref]$rect)
    $bmp = New-Object System.Drawing.Bitmap ($rect.R - $rect.L), ($rect.B - $rect.T)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($origin.X, $origin.Y, 0, 0, $bmp.Size)
    foreach ($p in $Rest) {
      $xy = $p.Split(",") | ForEach-Object { [int][Math]::Round([double]$_) }
      $c = $bmp.GetPixel($xy[0], $xy[1])
      "$($c.R),$($c.G),$($c.B)"
    }
    $g.Dispose(); $bmp.Dispose()
  }
  "move" { Move-To (To-Screen $Rest[0]) }
  "click" {
    $s = To-Screen $Rest[0]
    Hold $Rest[1] $true
    Move-To $s
    [FxWin]::mouse_event($LEFTDOWN, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 40
    [FxWin]::mouse_event($LEFTUP, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 40
    Hold $Rest[1] $false
  }
  "drag" {
    $a = To-Screen $Rest[0]; $b = To-Screen $Rest[1]
    $steps = if ($Rest.Count -gt 2) { [int]$Rest[2] } else { 16 }
    Move-To $a
    [FxWin]::mouse_event($LEFTDOWN, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 40
    for ($i = 1; $i -le $steps; $i++) {
      Move-To @([int]($a[0] + ($b[0] - $a[0]) * $i / $steps), [int]($a[1] + ($b[1] - $a[1]) * $i / $steps))
    }
    Start-Sleep -Milliseconds 40
    [FxWin]::mouse_event($LEFTUP, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 40
  }
  default { "usage: win.ps1 pixels x,y ... | click x,y [shift|ctrl|alt] | drag x0,y0 x1,y1 [steps] | move x,y" }
}
