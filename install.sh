#!/usr/bin/env sh
# Installs the atlas CLI and the atlas skill for Claude Code.
set -e
here="$(cd "$(dirname "$0")" && pwd)"
cargo install --path "$here/tools/atlas" --locked 2>/dev/null || cargo install --path "$here/tools/atlas"
dest="${CLAUDE_SKILLS_DIR:-$HOME/.claude/skills}/atlas"
mkdir -p "$dest"
cp -R "$here/skills/atlas/." "$dest/"
echo "installed: $(command -v atlas)  skill: $dest"
