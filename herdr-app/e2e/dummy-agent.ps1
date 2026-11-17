# dummy-agent.py's Windows stand-in for windows-app.ps1 (Windows PowerShell
# 5.1): reports its status to herdr-core (OSC 7501, app=pi), prints file paths
# like an agent's summary does, then waits.
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidUsingWriteHost', '', Justification = 'raw terminal output, escapes included')]
param()
$esc = [char]27
[Console]::Write("$esc]7501;state=idle:app=pi$esc\")
[Console]::Write("dummy agent in $((Get-Location).Path)`r`n")
[Console]::Write("I edited src/greet.js:1 and read package.json`r`n")
while ($true) { Start-Sleep -Seconds 1 }
