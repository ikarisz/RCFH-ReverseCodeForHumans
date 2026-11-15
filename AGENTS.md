# Agent instructions: CodeAtlas

Only when the user explicitly asks to explain or translate code into plain language (e.g. `/atlas <scope>`), follow [`skills/atlas/SKILL.md`](skills/atlas/SKILL.md). Never run it proactively; it exists to save tokens by explaining only what the user asks about.

```bash
cargo install --path tools/atlas             # once
atlas pending <folder|file|file#fn> [--changed]   # what needs explaining (exit 2 = nothing)
atlas write .atlas/tmp.json                  # save [{id|file|dir, summary, detail}]
atlas inline <scope> [--apply]               # optional: comments in code, AST-verified
atlas open                                   # local map app
```

MCP alternative: `atlas mcp` (tools `atlas_pending`, `atlas_write`, `atlas_status`, `atlas_show`, `atlas_scan`).
