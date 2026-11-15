# RCFH: Reverse Code For Humans

**CodeAtlas** — a local, open-source map of your codebase, explained in plain human language by your AI agent, only when you ask.

Sister project of [RSFA](https://github.com/ikarisz/RSFA-ReverseSkillsForAgents): RSFA reverses an app so **agents** can use it; RCFH reverses code so **humans** can understand it. Like Obsidian, but for code. A scanner draws the real structure of the project (folders → files → functions → who calls whom). Your AI agent (Claude Code, Codex, Cursor…) writes an explanation for each part that someone with **zero coding knowledge** can read. Explanations can stay beside the code or go *into* the code as comments, with a structural check that guarantees the code itself never changes.

```
/atlas src/payments      ← you type this when you don't understand something
```

| | |
|---|---|
| 🔌 **Wiring diagram** | `atlas open` shows every file as a box listing **each function inside it**. A wire runs from the exact function that makes a call to the exact function it calls. Callers are on the left and the functions they call are on the right. Hover a function to trace only its wires; click to read its explanation. It runs on `127.0.0.1` only and updates live while the agent writes. |
| 📖 **Reading view** | Each function shows its line number, a plain-language explanation, *uses / used by* links, the code beside it, and a jump back to its wires on the diagram. |
| 🧠 **Agent writes, tool measures** | tree-sitter builds the structure and call graph with no AI. The agent spends tokens only on explanations for exactly the code `atlas pending` hands it. |
| 💸 **On demand, never automatic** | Nothing runs by itself. You choose the scope: a folder, a file, a function, or `--changed` (new code in git). |
| ⏱️ **Knows when it's outdated** | Each explanation stores a hash of its code with comments and whitespace removed. When the code changes, the map marks it 🟠 and `/atlas --changed` re-explains only that. |
| 🛡️ **Safe in-code comments** | `atlas inline --apply` adds `[atlas]` comment blocks only after proving the comment-free AST is identical. `atlas uninline --apply` restores the files byte for byte. |
| 📓 **Obsidian too** | `atlas vault` exports `.atlas/vault/` with wikilinks between callers and callees. |
| 🔌 **MCP** | `atlas mcp` exposes `atlas_pending / atlas_write / atlas_status / atlas_show / atlas_scan`. |

Languages: JavaScript, TypeScript/TSX, Python, Rust, Go. Explanations are in Thai or English, at a beginner or developer level.

## Install

```bash
git clone https://github.com/ikarisz/RCFH-ReverseCodeForHumans
cd RCFH-ReverseCodeForHumans
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

## Find and explain errors

```
/atlas check                      find errors in the project and explain them in plain language
/atlas debug npm run build        run something that crashes, then explain why
(paste an error) what does this mean?
```

`atlas check` runs a built-in syntax scan plus whatever checkers the project already has (TypeScript `tsc`, Python, `ruff`, `cargo check`, `go vet`; `--tests` also runs the test suite). `atlas debug -- <command>` runs any command and reads its errors and stack traces. Every problem is pinned to **the function it is in**, with:

- the code around it, with the broken line marked
- who else is affected: the functions that call it
- the path a crash took through your code
- **commands you can run yourself**, both to see the problem and to confirm it's fixed

The agent then adds *what happened / why / how to fix* in plain language. You can read it here:

```bash
atlas problems                # in the terminal
atlas open                    # the Problems tab; broken functions are red on the wiring diagram
atlas trace checkout          # who calls this, and what it calls (what breaks if it breaks)
```

Example (`atlas problems`, Thai):

```
━━ 2. [ผิดพลาด] py/shop.py:6  (checkout)
เกิดอะไรขึ้น: ตอนกดชำระเงิน โปรแกรมหยุดทำงาน เพราะมีสินค้าชิ้นหนึ่งในตะกร้าที่ไม่มีราคา
ทำไม: ส่วน checkout รวมราคาสินค้าทุกชิ้น แต่ข้อมูลจาก load_cart มีชิ้นที่ไม่มีราคา (KeyError = หาช่องข้อมูลชื่อนี้ไม่เจอ)
วิธีแก้:
1. ตรวจว่าสินค้าทุกชิ้นใน load_cart มีราคาครบ (ต้นเหตุจริง)
2. หรือเปลี่ยน i["price"] เป็น i.get("price", 0)
>    6 |     return sum(i["price"] for i in items)
ลองรันเอง:
  $ python py/shop.py
  $ atlas debug -- python py/shop.py
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
check [scope...] [--tests]             find errors (syntax + tsc / python / ruff / cargo / go vet)
debug -- <command ...>                 run a command, map its errors / stack traces to the code
debug --log <file | ->                 same, from copied error output
problems [scope...]                    problems in plain language + commands to try
trace <symbol | name> [--depth 3]      callers and callees (impact of a change or failure)
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
├─ problems.json      last check / debug results (git-ignored)
└─ vault/             Obsidian export            (git-ignored)
```

## Roadmap

- Desktop shell (Tauri) around the same viewer
- "What happens when…" flow pages (click a button → step-by-step through the code)
- VS Code extension that shows explanations on hover
- More languages (Java/Kotlin, C#, PHP, Swift)

---

### ภาษาไทย

RCFH (CodeAtlas) คือแอปที่รันบนเครื่องของคุณเอง ใช้ดูแผนผังทั้งโปรเจกต์ (โฟลเดอร์ → ไฟล์ → ฟังก์ชัน → ใครเรียกใคร) โดยให้ AI agent เขียน **คำอธิบายเป็นภาษาคน** ที่คนไม่รู้โค้ดเลยก็อ่านเข้าใจ

- **ไม่ทำงานเองอัตโนมัติ** จะทำงานเฉพาะตอนพิมพ์ `/atlas <โฟลเดอร์|ไฟล์|--changed>` เท่านั้น จึงไม่เปลือง token
- **รู้ว่าคำอธิบายไหนล้าสมัย** ถ้าโค้ดเปลี่ยน แผนที่จะขึ้นสีส้ม และ `/atlas --changed` จะอธิบายเฉพาะส่วนที่เปลี่ยน
- **เขียนลงในโค้ดได้แบบไม่ทำให้โค้ดพัง** `atlas inline --apply` จะตรวจโครงสร้างโค้ดว่าเหมือนเดิม 100% ก่อนบันทึก และ `atlas uninline --apply` ลบออกได้หมด
- **แผนผังสายไฟ (`atlas open`)** ในกล่องของแต่ละไฟล์จะมีรายชื่อทุกฟังก์ชัน และมีสายลากจากฟังก์ชันที่เรียกไปหาฟังก์ชันที่ถูกเรียกแบบตัวต่อตัว ชี้ที่ชื่อเพื่อเน้นเฉพาะสายของมัน
- **ตรวจหา error แล้วอธิบายเป็นภาษาคน** ใช้ `/atlas check` ตรวจทั้งโปรเจกต์ หรือ `/atlas debug <คำสั่ง>` รันแล้วจับว่าพังตรงไหน ทุกปัญหาจะบอกว่าอยู่ในฟังก์ชันไหน มีส่วนไหนได้รับผลกระทบ เกิดอะไรขึ้น ทำไม แก้ยังไง และมี **คำสั่งให้ลองรันเอง** อ่านได้ด้วย `atlas problems` หรือแท็บ "ปัญหา" ในแอป
- **`atlas trace <ชื่อฟังก์ชัน>`** ดูว่าใครเรียกฟังก์ชันนี้ และมันเรียกอะไร เพื่อรู้ว่าถ้าแก้หรือพังจะกระทบตรงไหน
- เปิดแผนที่ด้วย `atlas open` หรือเปิดใน Obsidian ด้วย `atlas vault`

## License

MIT. Bundled fonts: IBM Plex Sans Thai and IBM Plex Mono, SIL Open Font License 1.1 ([`tools/atlas/web/fonts/OFL.txt`](tools/atlas/web/fonts/OFL.txt)).
