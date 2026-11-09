<#
.SYNOPSIS
  Free, local speech servers for ClawDot on native Windows: speech to text (whisper.cpp, port CLAWDOT_STT_PORT,
  internal) and text to speech (Piper behind a tiny OpenAI-compatible gateway on CLAWDOT_TTS_PORT, which also
  forwards transcriptions, so ClawDot's one speech.url is http://127.0.0.1:8082/v1). Nothing here calls a paid
  service. Guide: docs/guides/local-speech.md.
.DESCRIPTION
  .\scripts\dev\local-speech.ps1 setup    download the programs, models and voice (about 400 MB) to $env:CLAWDOT_SPEECH_DIR
  .\scripts\dev\local-speech.ps1 start    start both servers in the background (no window)
  .\scripts\dev\local-speech.ps1 status   say whether each answers
  .\scripts\dev\local-speech.ps1 stop     stop them
  .\scripts\dev\local-speech.ps1 test     speak a sentence and transcribe it back
  Environment: CLAWDOT_SPEECH_DIR (default %USERPROFILE%\clawdot-speech), CLAWDOT_STT_PORT (8081), CLAWDOT_TTS_PORT
  (8082), CLAWDOT_VOICE (en_US-joe-medium), CLAWDOT_STT_MODEL (base.en), CLAWDOT_STT_THREADS (4), CLAWDOT_BIND
  (127.0.0.1; use 0.0.0.0 only if a core in WSL2 must reach these servers, and mind the Windows firewall prompt).
#>
param([Parameter(Position = 0)][string]$Action = "")
$ErrorActionPreference = "Stop"

function Env($name, $default) { $v = [Environment]::GetEnvironmentVariable($name); if ($v) { $v } else { $default } }
$dir = Env "CLAWDOT_SPEECH_DIR" (Join-Path $env:USERPROFILE "clawdot-speech")
$sttPort = Env "CLAWDOT_STT_PORT" "8081"
$ttsPort = Env "CLAWDOT_TTS_PORT" "8082"
$voice = Env "CLAWDOT_VOICE" "en_US-joe-medium"
$model = Env "CLAWDOT_STT_MODEL" "base.en"
$threads = Env "CLAWDOT_STT_THREADS" "4"
$bind = Env "CLAWDOT_BIND" "127.0.0.1"
$whisperTag = "b5454"
$whisperSha256 = "6ba69e3482d7826214f90a6a9c84ca07782aec1e1d0c6a7c30c994fd5d816ccb"
$piperVersion = "1.8.0"
$server = Join-Path $dir "whisper\bin\Release\whisper-server.exe"
$py = Join-Path $dir "piper-venv\Scripts\python.exe"
$shim = Join-Path $PSScriptRoot "piper_openai_shim.py"

function Setup {
  New-Item -ItemType Directory -Force (Join-Path $dir "whisper\models"), (Join-Path $dir "voices") | Out-Null
  if (-not (Test-Path $server)) {
    Write-Host "downloading whisper.cpp $whisperTag (9 MB)"
    $zip = Join-Path $dir "whisper\w.zip"
    Invoke-WebRequest "https://github.com/ggml-org/whisper.cpp/releases/download/$whisperTag/whisper-bin-x64.zip" -OutFile $zip
    if ((Get-FileHash $zip -Algorithm SHA256).Hash.ToLower() -ne $whisperSha256) { throw "the whisper.cpp download does not match the pinned hash" }
    Expand-Archive $zip -DestinationPath (Join-Path $dir "whisper\bin") -Force
    Remove-Item $zip
  }
  $bin = Join-Path $dir "whisper\models\ggml-$model.bin"
  if (-not (Test-Path $bin)) {
    Write-Host "downloading the speech-to-text model ggml-$model.bin (base.en is 142 MB)"
    Invoke-WebRequest "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-$model.bin" -OutFile $bin
  }
  if (-not (Test-Path $py)) {
    if (-not (Get-Command python -ErrorAction SilentlyContinue)) { throw "Python 3.9 or newer is needed: winget install Python.Python.3.12" }
    python -m venv (Join-Path $dir "piper-venv")
    & $py -m pip install --quiet "piper-tts==$piperVersion"
  }
  if (-not (Test-Path (Join-Path $dir "voices\$voice.onnx"))) {
    Write-Host "downloading the voice $voice (about 61 MB)"
    & $py -m piper.download_voices --download-dir (Join-Path $dir "voices") $voice
  }
  Write-Host "setup done in $dir"
}

function Alive($name) {
  $f = Join-Path $dir "$name.pid"
  if (-not (Test-Path $f)) { return $false }
  [bool](Get-Process -Id ([int](Get-Content $f)) -ErrorAction SilentlyContinue)
}

function Up($port) {
  try { Invoke-WebRequest "http://127.0.0.1:$port/health" -TimeoutSec 2 -UseBasicParsing | Out-Null; $true } catch { $false }
}

function Status {
  $ok = $true
  if (Up $sttPort) { Write-Host "speech to text: up   http://127.0.0.1:$sttPort/v1 (ggml-$model)" } else { Write-Host "speech to text: DOWN (log: $dir\stt.log)"; $ok = $false }
  if (Up $ttsPort) { Write-Host "text to speech: up   ($voice); ClawDot's speech.url is http://127.0.0.1:$ttsPort/v1 (both directions)" } else { Write-Host "text to speech: DOWN (log: $dir\tts.log)"; $ok = $false }
  $ok
}

function Start-One($name, $exe, $argList) {
  if (Alive $name) { Write-Host "$name already running"; return }
  $p = Start-Process -FilePath $exe -ArgumentList $argList -WindowStyle Hidden -PassThru `
    -RedirectStandardOutput (Join-Path $dir "$name.log") -RedirectStandardError (Join-Path $dir "$name.err.log")
  $p.Id | Set-Content (Join-Path $dir "$name.pid")
}

function Start-Servers {
  if (-not (Test-Path $server)) { throw "run setup first" }
  # Quote every argument: Start-Process joins them with spaces and the path of the model may contain some.
  Start-One "stt" $server @("-m", "`"$dir\whisper\models\ggml-$model.bin`"", "--host", $bind, "--port", $sttPort, "-t", $threads, "--inference-path", "/v1/audio/transcriptions")
  Start-One "tts" $py @("`"$shim`"", "--voice-dir", "`"$dir\voices`"", "--voice", $voice, "--host", $bind, "--port", $ttsPort, "--stt-url", "http://127.0.0.1:$sttPort")
  for ($i = 0; $i -lt 60; $i++) {
    if ((Up $sttPort) -and (Up $ttsPort)) { [void](Status); return }
    Start-Sleep 1
  }
  [void](Status)
  throw "a server did not come up in 60 seconds; read $dir\stt.err.log and $dir\tts.err.log"
}

function Stop-Servers {
  foreach ($n in "stt", "tts") {
    if (Alive $n) { Stop-Process -Id ([int](Get-Content (Join-Path $dir "$n.pid"))) -Force; Write-Host "stopped $n" }
    Remove-Item (Join-Path $dir "$n.pid") -ErrorAction SilentlyContinue
  }
}

function Test-Roundtrip {
  $wav = Join-Path $dir "test.wav"
  $body = '{"model":"piper","voice":"x","input":"Hello from ClawDot. The weather is fine.","response_format":"wav"}'
  Invoke-WebRequest "http://127.0.0.1:$ttsPort/v1/audio/speech" -Method Post -ContentType "application/json" -Body $body -OutFile $wav
  Write-Host ("spoke {0} bytes of WAV to {1}" -f (Get-Item $wav).Length, $wav)
  $conv = Join-Path $dir "test16k.wav"
  & $py -c "import sys,wave,numpy as np;r=wave.open(sys.argv[1]);x=np.frombuffer(r.readframes(r.getnframes()),dtype=np.int16).astype(float);n=int(len(x)*16000/r.getframerate());y=np.interp(np.linspace(0,len(x)-1,n),np.arange(len(x)),x).astype(np.int16);w=wave.open(sys.argv[2],'wb');w.setnchannels(1);w.setsampwidth(2);w.setframerate(16000);w.writeframes(y.tobytes());w.close()" $wav $conv
  Write-Host "heard back:"
  & curl.exe -s "http://127.0.0.1:$ttsPort/v1/audio/transcriptions" -F "file=@$conv" -F model=whisper-1 -F response_format=json
  Write-Host ""
}

switch ($Action) {
  "setup" { Setup }
  "start" { Start-Servers }
  "status" { if (-not (Status)) { exit 1 } }
  "stop" { Stop-Servers }
  "test" { Test-Roundtrip }
  default { Get-Help $PSCommandPath; exit 2 }
}
