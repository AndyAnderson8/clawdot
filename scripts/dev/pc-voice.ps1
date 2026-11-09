<#
.SYNOPSIS
  One command for a live spoken loop on a Windows PC: speech servers (WSL2), the ClawDot core (WSL2) and the
  device program with the wake word (Windows). Guide: docs/guides/pc-voice-loop.md.
.DESCRIPTION
  .\scripts\dev\pc-voice.ps1 -ListDevices                       list microphone names (needs ffmpeg)
  .\scripts\dev\pc-voice.ps1 -Mic "Microphone (USB Audio)"      run the loop; Ctrl+C or q then Enter leaves
  Keys in this window: Enter = talk now (push-to-talk), m = mute, s = stop speaking, q = quit.
  First run builds the programs (a few minutes). OPENROUTER_API_KEY is passed to the core from your Windows
  user environment. Nothing is sent anywhere except the language model request.
#>
param(
  [string]$Mic = "",
  [string]$Board = "desktop",
  [string]$Config = "",
  [string]$WakeModel = "",
  [double]$Sensitivity = 0.5,
  [switch]$NoWake,
  [switch]$NoSpeechServers,
  [switch]$KeepServers,
  [switch]$ListDevices,
  [switch]$Rebuild,
  [int]$Turns = 0,
  [string]$Distro = "Ubuntu"
)
$ErrorActionPreference = "Stop"
$repo = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$exe = Join-Path $repo "target\windows\clawdot-device.exe"
if (-not $Config) { $Config = Join-Path $repo "clawdot-pc.json" }
if (-not $WakeModel) { $WakeModel = Join-Path $repo "crates\wake\tests\golden\models\okay_nabu.tflite" }

# wsl.exe passes the command through a shell, which eats backslashes: give wslpath forward slashes.
function Wsl-Path($p) { (& wsl.exe -d $Distro -- wslpath -a (([string]$p) -replace '\\', '/')).Trim() }
function Wsl-Run($script) { & wsl.exe -d $Distro -- bash -lc $script; if ($LASTEXITCODE -ne 0) { throw "WSL command failed: $script" } }

if ($ListDevices) {
  if (-not (Get-Command ffmpeg -ErrorAction SilentlyContinue)) { throw "ffmpeg is not installed: winget install Gyan.FFmpeg, then open a new terminal" }
  & ffmpeg -hide_banner -list_devices true -f dshow -i dummy 2>&1 | Select-String "audio"
  return
}

# 1. What we need
if ($Board -eq "desktop") {
  foreach ($t in "ffmpeg", "ffplay") {
    if (-not (Get-Command $t -ErrorAction SilentlyContinue)) { throw "$t is not installed: winget install Gyan.FFmpeg, then open a new terminal" }
  }
  if (-not $Mic) { throw "-Mic is needed (run with -ListDevices to see the names), for example -Mic `"Microphone (USB Audio Device)`"" }
}
if (-not (Test-Path $Config)) {
  Copy-Item (Join-Path $repo "docs\guides\examples\local-speech.json") $Config
  Write-Host "created $Config from the example; edit the llm section if you want another model"
}
if (-not $NoWake -and -not (Test-Path $WakeModel)) { throw "wake word model not found: $WakeModel (see the guide for where to download one)" }

$wslRepo = Wsl-Path $repo
$wslConfig = Wsl-Path $Config

# 2. Build what is missing
if ($Rebuild -or -not (Test-Path $exe)) {
  Write-Host "building the Windows programs from WSL2 (first time takes a few minutes)"
  Wsl-Run ". ~/.clawdot-env.sh && cd '$wslRepo' && scripts/dev/build-windows.sh"
}
Wsl-Run ". ~/.clawdot-env.sh && cd '$wslRepo' && cargo build --release -p clawdot-core 2>&1 | tail -2"
# The build directory set by scripts/setup-wsl.sh (~/.clawdot-env.sh).
$core = "~/.cache/clawdot-target/release/clawdot"

# 3. Speech servers (WSL2)
if (-not $NoSpeechServers) {
  Wsl-Run "cd '$wslRepo' && scripts/dev/local-speech.sh setup && scripts/dev/local-speech.sh start"
}

# 4. The core (WSL2), hidden, logging to clawdot-core.log
$env:WSLENV = "OPENROUTER_API_KEY/u:" + $env:WSLENV
$log = Join-Path $repo "clawdot-core.log"
$coreProc = Start-Process wsl.exe -ArgumentList @("-d", $Distro, "--", "bash", "-lc", "cd '$wslRepo' && exec $core --config '$wslConfig' --no-console --echo-speech") `
  -WindowStyle Hidden -PassThru -RedirectStandardOutput $log -RedirectStandardError ($log + ".err")
try {
  $up = $false
  for ($i = 0; $i -lt 60 -and -not $up; $i++) {
    Start-Sleep 1
    try { $c = New-Object Net.Sockets.TcpClient; $c.Connect("127.0.0.1", 10400); $c.Close(); $up = $true } catch {}
    if ($coreProc.HasExited) { throw "the core stopped at start; read $log.err" }
  }
  if (-not $up) { throw "the core did not open 127.0.0.1:10400 in 60 seconds; read $log.err" }
  Write-Host "core is up (log: $log)"

  # 5. The device program, in this window so the keys reach it
  $deviceArgs = @("--board", $Board, "--core", "127.0.0.1:10400")
  if ($Mic) { $deviceArgs += @("--mic-device", $Mic) }
  if ($Turns -gt 0) { $deviceArgs += @("--turns", "$Turns") }
  if (-not $NoWake) { $deviceArgs += @("--wake-model", $WakeModel, "--wake-sensitivity", "$Sensitivity") }
  Write-Host "Enter = talk now, m = mute, s = stop speaking, q = quit"
  & $exe @deviceArgs
}
finally {
  if (-not $coreProc.HasExited) { & wsl.exe -d $Distro -- pkill -f "$wslConfig" | Out-Null }
  if (-not $NoSpeechServers -and -not $KeepServers) { & wsl.exe -d $Distro -- bash -lc "cd '$wslRepo' && scripts/dev/local-speech.sh stop" | Out-Null }
}
