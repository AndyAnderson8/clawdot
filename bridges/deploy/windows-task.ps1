# Run a bridge at logon on Windows with Task Scheduler. Run once, in PowerShell, from the repository root:
#   .\bridges\deploy\windows-task.ps1 -Bridge ntfy -Config C:\clawdot\ntfy.toml
#
# The secrets are NOT in the task. Set them as user environment variables first (they are stored for your
# account, not in the task definition or this script):
#   [Environment]::SetEnvironmentVariable("CLAWDOT_TOKEN", "cdt_...", "User")
#   [Environment]::SetEnvironmentVariable("NTFY_TOKEN", "tk_...", "User")        # ntfy, if protected
#   [Environment]::SetEnvironmentVariable("DISCORD_TOKEN", "...", "User")        # discord
param(
    [Parameter(Mandatory)][ValidateSet("ntfy", "discord_bridge")][string]$Bridge,
    [Parameter(Mandatory)][string]$Config,
    [string]$Python = "python"
)
$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$action = New-ScheduledTaskAction -Execute $Python `
    -Argument "-m clawdot_bridges.$Bridge --config `"$Config`"" -WorkingDirectory $root
$trigger = New-ScheduledTaskTrigger -AtLogOn
$settings = New-ScheduledTaskSettingsSet -RestartCount 5 -RestartInterval (New-TimeSpan -Minutes 1) `
    -ExecutionTimeLimit ([TimeSpan]::Zero) -StartWhenAvailable -AllowStartIfOnBatteries
Register-ScheduledTask -TaskName "ClawDot $Bridge bridge" -Action $action -Trigger $trigger `
    -Settings $settings -Description "Forwards $Bridge messages to ClawDot's announce API" -Force
Write-Host "Registered. Start it now with: Start-ScheduledTask -TaskName 'ClawDot $Bridge bridge'"
