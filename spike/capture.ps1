# SPIKE ONLY — screen capture of OUR devboule window (never CDP).
# DPI-AWARE process: without SetProcessDPIAware, GetWindowRect and
# CopyFromScreen return virtualized (scaled) coordinates on a 125/150% display.
# Finds the devboule.exe whose path contains devboule-v2-spike (ours; the
# other running Devboule is a different worktree), brings it to the front
# pinned on top so no other window occludes the capture, copies that screen
# region to a PNG, prints JSON with rects + DPI + scale.
param(
  [Parameter(Mandatory = $true)][string]$OutPng,
  [string]$ProcPathHint = "devboule-v2-spike",
  [int]$SettleMs = 350
)
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public class SpikeWin {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT r);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr hWnd, out RECT r);
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr hWnd, ref POINT p);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
}
'@
[void][SpikeWin]::SetProcessDPIAware()

$proc = Get-Process -Name devboule -ErrorAction SilentlyContinue |
  Where-Object { $_.Path -and $_.Path -like "*$ProcPathHint*" } |
  Select-Object -First 1
if (-not $proc) { Write-Error "no devboule.exe matching *$ProcPathHint*"; exit 3 }
$hwnd = $proc.MainWindowHandle
if ($hwnd -eq [IntPtr]::Zero) { Write-Error "pid $($proc.Id) has no main window"; exit 4 }

# Pin on top + foreground so nothing overlaps the capture region.
$HWND_TOPMOST = [IntPtr]::new(-1)
$SWP_NOSIZE = 0x0001; $SWP_NOMOVE = 0x0002; $SWP_NOACTIVATE = 0x0010
[void][SpikeWin]::SetWindowPos($hwnd, $HWND_TOPMOST, 0, 0, 0, 0, ($SWP_NOSIZE -bor $SWP_NOMOVE -bor $SWP_NOACTIVATE))
[void][SpikeWin]::SetForegroundWindow($hwnd)
[void][SpikeWin]::BringWindowToTop($hwnd)
Start-Sleep -Milliseconds $SettleMs

$wr = New-Object SpikeWin+RECT
[void][SpikeWin]::GetWindowRect($hwnd, [ref]$wr)
$cr = New-Object SpikeWin+RECT
[void][SpikeWin]::GetClientRect($hwnd, [ref]$cr)
$pt = New-Object SpikeWin+POINT
$pt.X = 0; $pt.Y = 0
[void][SpikeWin]::ClientToScreen($hwnd, [ref]$pt)
$dpi = [SpikeWin]::GetDpiForWindow($hwnd)

$w = $wr.Right - $wr.Left
$h = $wr.Bottom - $wr.Top
$bmp = New-Object System.Drawing.Bitmap($w, $h)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($wr.Left, $wr.Top, 0, 0, (New-Object System.Drawing.Size($w, $h)))
$bmp.Save($OutPng, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()

[pscustomobject]@{
  pid          = $proc.Id
  hwnd         = $hwnd.ToString()
  outer        = @{ left = $wr.Left; top = $wr.Top; right = $wr.Right; bottom = $wr.Bottom; w = $w; h = $h }
  clientScreen = @{ x = $pt.X; y = $pt.Y; w = $cr.Right; h = $cr.Bottom }
  dpi          = $dpi
  scale        = [math]::Round($dpi / 96.0, 4)
  utcMs        = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
  png          = (Resolve-Path $OutPng).Path
} | ConvertTo-Json -Compress
