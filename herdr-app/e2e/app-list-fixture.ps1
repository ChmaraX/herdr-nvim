#!/usr/bin/env pwsh
# Builds the `app-list` fixture in an empty folder, the same on every OS, so
# `herdr-nvim app-list <folder>` can be compared with
# fixtures/app-list.golden.json (as JSON). The same tree as src/app/list.rs's
# test: 600 entries and a .git.
#
#   pwsh herdr-app/e2e/app-list-fixture.ps1 <empty folder>
#
# Links: symlinks (relative), on Windows where this user may make them. Without
# that right, a junction stands in for the folder link and for the broken one
# (its target made, then removed), and a hard link for the file link: the
# listing reads all of them the same way.
param([Parameter(Mandatory)] [string]$At)
$ErrorActionPreference = 'Stop'

New-Item -ItemType Directory -Force -Path $At | Out-Null
$At = (Resolve-Path -LiteralPath $At).Path
if (@(Get-ChildItem -LiteralPath $At -Force).Count) { throw "$At is not empty" }
Push-Location -LiteralPath $At
try {
  git init -q .
  if ($LASTEXITCODE) { throw 'git init failed' }
  [IO.File]::WriteAllText("$At/.gitignore", "build/`n*.log`n")
  foreach ($name in '.hidden-note', 'app.log', 'notes.md', 'README.md') { [IO.File]::WriteAllText("$At/$name", 'x') }
  foreach ($dir in 'build', 'src', 'zz-folder') { New-Item -ItemType Directory -Path "$At/$dir" | Out-Null }
  [IO.File]::WriteAllText("$At/build/out.bin", 'x')
  [IO.File]::WriteAllText("$At/src/main.rs", 'x')
  foreach ($i in 0..588) { [IO.File]::WriteAllText(("$At/w{0:d3}.txt" -f $i), 'x') }

  function Link([string]$Name, [string]$Target, [string]$Fallback) {
    if (-not $IsWindows) {
      ln -s $Target "$At/$Name"
      if ($LASTEXITCODE) { throw "ln -s $Target $Name failed" }
      return
    }
    try {
      New-Item -ItemType SymbolicLink -Path "$At/$Name" -Target $Target | Out-Null
    } catch {
      switch ($Fallback) {
        'junction' { New-Item -ItemType Junction -Path "$At/$Name" -Target "$At/$Target" | Out-Null }
        'hardlink' { New-Item -ItemType HardLink -Path "$At/$Name" -Target "$At/$Target" | Out-Null }
        'broken' {
          New-Item -ItemType Directory -Path "$At/$Target" | Out-Null
          New-Item -ItemType Junction -Path "$At/$Name" -Target "$At/$Target" | Out-Null
          Remove-Item -LiteralPath "$At/$Target"
        }
      }
    }
  }
  Link -Name 'link-dir' -Target 'src' -Fallback 'junction'
  Link -Name 'link-file' -Target 'notes.md' -Fallback 'hardlink'
  Link -Name 'broken' -Target 'missing' -Fallback 'broken'
} finally {
  Pop-Location
}

$count = @(Get-ChildItem -LiteralPath $At -Force | Where-Object Name -ne '.git').Count
if ($count -ne 600) { throw "fixture has $count entries besides .git, not 600" }
