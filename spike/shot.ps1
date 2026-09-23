# SPIKE ONLY — question (a): launch N, wait for the child to load, capture.
# Usage: powershell -File spike/shot.ps1 -Tag a1
# Assumes the app is already running (launched separately) and spike-shots exists.
param(
  [Parameter(Mandatory = $true)][string]$Tag,
  [string]$OutDir = "C:\Users\gualt\Desktop\New devboule\scout\browser-tabs\spike-shots"
)
$ErrorActionPreference = "Stop"
if (-not (Test-Path $OutDir)) { New-Item -ItemType Directory -Path $OutDir | Out-Null }
$png = Join-Path $OutDir "$Tag.png"
$json = & powershell -NoProfile -ExecutionPolicy Bypass -File "$PSScriptRoot\capture.ps1" -OutPng $png
$json
