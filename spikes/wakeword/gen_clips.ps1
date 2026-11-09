param([string]$out)
# Generates test clips with local Windows SAPI voices (16 kHz mono 16-bit WAV). Positives: wake phrases; negatives: other speech.
Add-Type -AssemblyName System.Speech
New-Item -ItemType Directory -Force $out | Out-Null
$fmt = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo(16000, [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen, [System.Speech.AudioFormat.AudioChannel]::Mono)
$voices = @('Microsoft David Desktop','Microsoft Zira Desktop','Microsoft Mark')
$pos = @{ 'hey_jarvis'=@('hey jarvis','hey jarvis what is the weather','hey, jarvis, set a timer','jarvis'); 'okay_nabu'=@('okay nabu','okay nabu turn on the lights'); 'alexa'=@('alexa','alexa play some music'); 'hey_mycroft'=@('hey mycroft','hey mycroft what time is it') }
$neg = @('the quick brown fox jumps over the lazy dog','what is the weather like in london tomorrow','please turn on the kitchen lights','set a timer for ten minutes','I think the nabu is a type of bird','hey there how are you doing today','okay lets go to the market and buy some bread','a jar of marmalade is on the shelf','she sells sea shells by the sea shore','my mother and my mercury car are in the garage','play some jazz music in the living room','the lexicon of modern english is vast','hey judy come over here please','jarvis is not here today he went home','call me back when you are free tonight')
$i = 0
foreach ($v in $voices) {
  $s = New-Object System.Speech.Synthesis.SpeechSynthesizer
  $s.SelectVoice($v)
  foreach ($k in $pos.Keys) { foreach ($t in $pos[$k]) { $i++; $s.SetOutputToWaveFile("$out\pos__${k}__$i.wav", $fmt); $s.Speak($t); $s.SetOutputToNull() } }
  foreach ($t in $neg) { $i++; $s.SetOutputToWaveFile("$out\neg__$i.wav", $fmt); $s.Speak($t); $s.SetOutputToNull() }
  $s.Dispose()
}
"done $i"
