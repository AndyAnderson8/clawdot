# Forgot the portal password on an Echo Dot that has no shell for you: reset it over the USB serial cable.
#   scripts/dev/dot-reset-admin.ps1 [-Port COM5]
# Plug the Dot's USB cable into this computer first. The script signs ClawDot's administrator out (the same as
# `clawdot --reset-admin`: only the password is removed; timers, memory, inputs and settings are kept), then restarts
# ClawDot so the portal asks for a new password. The first person on your network to open the page chooses it, so
# open it straight away.
param(
  [string]$Port = "COM5",
  [string]$Dir = "/data/clawdot"
)
$ErrorActionPreference = "Stop"
Write-Host "This removes the portal password on the Dot connected to $Port. Timers, memory, inputs and settings are kept."
if ((Read-Host "Type yes to continue") -ne "yes") { Write-Host "Nothing was changed."; exit 1 }

$sp = New-Object System.IO.Ports.SerialPort $Port, 115200, None, 8, One
$sp.ReadTimeout = 500
$sp.Open()
function Send([string]$cmd, [int]$wait = 4) {
  $mark = "DONE" + [guid]::NewGuid().ToString("N").Substring(0, 8)
  $sp.Write("$cmd; echo $mark`n")
  $buf = ""
  $until = (Get-Date).AddSeconds($wait + 20)
  while ((Get-Date) -lt $until) {
    try { $buf += $sp.ReadExisting() } catch {}
    if (([regex]::Matches($buf, $mark)).Count -ge 2) { break }
    Start-Sleep -Milliseconds 150
  }
  ($buf -replace "`r", "").Trim()
}
try {
  $sp.DiscardInBuffer()
  $sp.Write("`n")
  Start-Sleep -Milliseconds 400
  $null = $sp.ReadExisting()
  Write-Host (Send "cd $Dir && ./clawdot --config clawdot.json --reset-admin")
  # The running ClawDot keeps the old password in memory: restart it (the supervisor starts it again).
  $null = Send 'kill $(pidof clawdot)'
  Write-Host ""
  Write-Host "Done. In about 15 seconds open the Dot's portal page (http://<its address>:3335) and choose a new password."
} finally {
  $sp.Close()
}
