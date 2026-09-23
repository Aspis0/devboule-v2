# SPIKE ONLY — capture N frames of OUR window back-to-back with PRE-capture
# timestamps, for lag measurement: which frame first shows the new child
# position after a move.
# Usage: powershell -File spike/loop.ps1 -OutDir <dir> -Count 10 [-ProcPathHint x]
param(
  [Parameter(Mandatory = $true)][string]$OutDir,
  [int]$Count = 10,
  [string]$ProcPathHint = "devboule-v2-spike"
)
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public class SpikeWin2 {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT r);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr hWnd);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
'@
[void][SpikeWin2]::SetProcessDPIAware()
if (-not (Test-Path $OutDir)) { New-Item -ItemType Directory -Path $OutDir | Out-Null }

$proc = Get-Process -Name devboule -ErrorAction SilentlyContinue |
  Where-Object { $_.Path -and $_.Path -like "*$ProcPathHint*" } | Select-Object -First 1
if (-not $proc) { Write-Error "no our devboule.exe"; exit 3 }
$hwnd = $proc.MainWindowHandle

$frames = @()
for ($i = 0; $i -lt $Count; $i++) {
  $wr = New-Object SpikeWin2+RECT
  [void][SpikeWin2]::GetWindowRect($hwnd, [ref]$wr)
  $tPre = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
  $w = $wr.Right - $wr.Left; $h = $wr.Bottom - $wr.Top
  $bmp = New-Object System.Drawing.Bitmap($w, $h)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($wr.Left, $wr.Top, 0, 0, (New-Object System.Drawing.Size($w, $h)))
  $file = Join-Path $OutDir ("f{0:d2}.png" -f $i)
  $bmp.Save($file, [System.Drawing.Imaging.ImageFormat]::Png)
  $g.Dispose(); $bmp.Dispose()
  $frames += @{ i = $i; tPre = $tPre; left = $wr.Left; top = $wr.Top;
                clientDx = $null; file = $file }
}
[pscustomobject]@{ frames = $frames } | ConvertTo-Json -Compress -Depth 4
