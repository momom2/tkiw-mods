# Keep the machine awake for an unattended session.
#
# Written for overnight autonomous work: launching the game, reading logs and
# building take hours, and a machine that sleeps halfway through wastes all of it.
#
#   powershell -ExecutionPolicy Bypass -File keep-awake.ps1 -Hours 8
#
# Asserts ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED via
# SetThreadExecutionState, which is the documented way to say "I am doing something,
# do not sleep" -- no synthetic keystrokes, nothing that could land in a game window.
#
# The assertion belongs to the *thread* that makes it, so this script has to stay
# alive; when it exits, or is killed, Windows reverts to normal power behaviour by
# itself. That is deliberate: a crash here must not leave the machine unable to sleep.
#
# It re-asserts on a timer as well. One call is enough in principle, but a re-assert
# costs nothing and covers the case where something else in the session resets the
# state.

param(
    [double]$Hours = 8,
    [int]$IntervalSeconds = 60,
    [string]$LogPath = "$PSScriptRoot\keep-awake.log"
)

Add-Type -Namespace Power -Name Util -MemberDefinition @'
[DllImport("kernel32.dll", SetLastError = true)]
public static extern uint SetThreadExecutionState(uint esFlags);
'@

$ES_CONTINUOUS        = [uint32]0x80000000
$ES_SYSTEM_REQUIRED   = [uint32]0x00000001
$ES_DISPLAY_REQUIRED  = [uint32]0x00000002
$flags = $ES_CONTINUOUS -bor $ES_SYSTEM_REQUIRED -bor $ES_DISPLAY_REQUIRED

$deadline = (Get-Date).AddHours($Hours)
"[$(Get-Date -Format 'HH:mm:ss')] keep-awake: holding the machine awake until $($deadline.ToString('yyyy-MM-dd HH:mm:ss'))" |
    Tee-Object -FilePath $LogPath -Append

try {
    while ((Get-Date) -lt $deadline) {
        $previous = [Power.Util]::SetThreadExecutionState($flags)
        if ($previous -eq 0) {
            "[$(Get-Date -Format 'HH:mm:ss')] keep-awake: SetThreadExecutionState failed (error $([System.Runtime.InteropServices.Marshal]::GetLastWin32Error()))" |
                Tee-Object -FilePath $LogPath -Append
        }
        Start-Sleep -Seconds $IntervalSeconds
    }
    "[$(Get-Date -Format 'HH:mm:ss')] keep-awake: deadline reached, releasing." |
        Tee-Object -FilePath $LogPath -Append
}
finally {
    # Hand power management back, whether we finished or were interrupted.
    [void][Power.Util]::SetThreadExecutionState($ES_CONTINUOUS)
}
