# SPIKE ONLY — list msedgewebview2.exe processes: pid, parent, user-data-dir
# and remote-debugging args from the command line (never kills anything).
$procs = Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'"
$rows = foreach ($p in $procs) {
  $cl = if ($p.CommandLine) { $p.CommandLine } else { "" }
  $udd = ""
  if ($cl -match '--user-data-dir="?([^"\s]+)"?') { $udd = $Matches[1] }
  if ($cl -match '--user-data-folder="?([^"\s]+)"?') { $udd = $Matches[1] }
  $rdp = ""
  if ($cl -match '--remote-debugging-port=(\d+)') { $rdp = $Matches[1] }
  $type = "browser"
  if ($cl -match '--type=(\S+)') { $type = $Matches[1] }
  [pscustomobject]@{
    pid = $p.ProcessId; parent = $p.ParentProcessId; type = $type
    userDataDir = $udd; debugPort = $rdp
  }
}
$rows | ConvertTo-Json -Compress
