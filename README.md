# CodeAtlas

**A local, open-source map of your codebase, explained in plain human language by your AI agent, only when you ask.**

Like Obsidian, but for code. A scanner draws the real structure of the project (folders → files → functions → who calls whom). Your AI agent (Claude Code, Codex, Cursor…) writes an explanation for each part that someone with **zero coding knowledge** can read. Explanations can stay beside the code or go *into* the code as comments, with a structural check that guarantees the code itself never changes.

```
/atlas src/payments      ← you type this when you don't understand something
```

| | |
|---|---|
| 🗺️ **Map app** | `atlas open` gives you a zoomable map of folders and files, function cards with explanation, "uses / used by" links, the code beside it, and search. It runs on `127.0.0.1` only and updates live while the agent writes. |
| 🧠 **Agent writes, tool measures** | tree-sitter builds the structure and call graph with no AI. The agent spends tokens only on explanations for exactly the code `atlas pending` hands it. |
| 💸 **On demand, never automatic** | Nothing runs by itself. You choose the scope: a folder, a file, a function, or `--changed` (new code in git). |
| ⏱️ **Knows when it's outdated** | Each explanation stores a hash of its code with comments and whitespace removed. When the code changes, the map marks it 🟠 and `/atlas --changed` re-explains only that. |
| 🛡️ **Safe in-code comments** | `atlas inline --apply` adds `[atlas]` comment blocks only after proving the comment-free AST is identical. `atlas uninline --apply` restores the files byte for byte. |
| 📓 **Obsidian too** | `atlas vault` exports `.atlas/vault/` with wikilinks between callers and callees. |
| 🔌 **MCP** | `atlas mcp` exposes `atlas_pending / atlas_write / atlas_status / atlas_show / atlas_scan`. |

Languages: JavaScript, TypeScript/TSX, Python, Rust, Go. Explanations are in Thai or English, at a beginner or developer level.

## Install

```bash
git clone <this repo> CodeAtlas && cd CodeAtlas
./install.sh          # Windows: powershell -ExecutionPolicy Bypass -File install.ps1
```

This builds `atlas` (`cargo install`) and copies the skill to `~/.claude/skills/atlas`, so `/atlas` works in Claude Code. For other agents, see [`AGENTS.md`](AGENTS.md). Requires the Rust toolchain.

## Use

In your project:

```bash
atlas init --lang th --level beginner   # once
```

Then, in your agent, whenever you want something explained:

```
/atlas src/checkout           a folder
/atlas src/cart.ts#applyCoupon   one function
/atlas --changed              the code that was just added or changed
```

Look at it:

```bash
atlas open                    # map app in your browser
atlas show src/cart.ts        # or read in the terminal
```

Optionally put the explanations into the code:

```bash
atlas inline src/checkout           # preview (dry run)
atlas inline src/checkout --apply   # write [atlas] comments, AST-verified
atlas uninline --apply              # remove them all again
```

## Commands

```
init [--lang th|en] [--level beginner|dev]
scan                                   structure only, no AI
status [scope...]                      explained / outdated / missing per folder
pending [scope...] [--changed] [--force] [--budget LINES] [--limit N]
write <file.json | ->
show <file | file#symbol | folder/>
inline [scope...] [--apply]
uninline [scope...] [--apply]
vault
open [--port 7420] [--no-browser]
mcp
```

## What it stores

```
.atlas/
├─ config.json        language + level           (commit)
├─ notes/**.json      the explanations           (commit — they are the documentation)
├─ graph.json         scanned structure          (git-ignored, rebuilt any time)
└─ vault/             Obsidian export            (git-ignored)
```

## Roadmap

- Desktop shell (Tauri) around the same viewer
- "What happens when…" flow pages (click a button → step-by-step through the code)
- VS Code extension that shows explanations on hover
- More languages (Java/Kotlin, C#, PHP, Swift)

---

### ภาษาไทย

CodeAtlas คือแอปที่รันบนเครื่องของคุณเอง ใช้ดูแผนผังทั้งโปรเจกต์ (โฟลเดอร์ → ไฟล์ → ฟังก์ชัน → ใครเรียกใคร) โดยให้ AI agent เขียน **คำอธิบายเป็นภาษาคน** ที่คนไม่รู้โค้ดเลยก็อ่านเข้าใจ

- **ไม่ทำงานเองอัตโนมัติ** จะทำงานเฉพาะตอนพิมพ์ `/atlas <โฟลเดอร์|ไฟล์|--changed>` เท่านั้น จึงไม่เปลือง token
- **รู้ว่าคำอธิบายไหนล้าสมัย** ถ้าโค้ดเปลี่ยน แผนที่จะขึ้นสีส้ม และ `/atlas --changed` จะอธิบายเฉพาะส่วนที่เปลี่ยน
- **เขียนลงในโค้ดได้แบบไม่ทำให้โค้ดพัง** `atlas inline --apply` จะตรวจโครงสร้างโค้ดว่าเหมือนเดิม 100% ก่อนบันทึก และ `atlas uninline --apply` ลบออกได้หมด
- เปิดแผนที่ด้วย `atlas open` หรือเปิดใน Obsidian ด้วย `atlas vault`

## License

MIT
