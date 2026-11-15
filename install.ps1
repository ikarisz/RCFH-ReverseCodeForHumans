# Installs the atlas CLI and the atlas skill for Claude Code (Windows).
$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
cargo install --path "$here\tools\atlas"
if ($LASTEXITCODE -ne 0) { throw "cargo install failed" }
$skills = if ($env:CLAUDE_SKILLS_DIR) { $env:CLAUDE_SKILLS_DIR } else { Join-Path $HOME '.claude\skills' }
$dest = Join-Path $skills 'atlas'
New-Item -ItemType Directory -Force $dest | Out-Null
Copy-Item -Recurse -Force "$here\skills\atlas\*" $dest
Write-Host "installed: $((Get-Command atlas).Source)  skill: $dest"
