@echo off
rem SPIKE ONLY — launcher for `tauri dev` under cmd.exe (agent async jobs).
set "PATH=C:\Program Files\nodejs;C:\Users\gualt\.cargo\bin;C:\Users\gualt\AppData\Roaming\npm;%PATH%"
cd /d "C:\Users\gualt\Desktop\New devboule\devboule-v2-spike"
echo CMDLauncher pid=%RANDOM% >&2
node node_modules/@tauri-apps/cli/tauri.js dev
