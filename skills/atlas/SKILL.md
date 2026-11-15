---
name: atlas
description: Translate code into plain human language that someone with zero coding knowledge can read, keep a local map of the project (CodeAtlas), and find and explain errors in plain language with commands the user can run themselves. Use ONLY when the user explicitly asks, e.g. "/atlas src/payments", "/atlas --changed", "/atlas check", "/atlas debug npm run dev", "อธิบายโค้ดนี้ให้หน่อย", "แปลโค้ดเป็นภาษาคน", "ตรวจหา error", "ทำไมมันพัง", a pasted error or stack trace with "what does this mean", "explain this code for a non-programmer". Never run it on your own initiative or after edits; it spends tokens only on request. Drives the `atlas` CLI, which finds what needs explaining or fixing; you write the explanations.
---

# CodeAtlas: explain code on request

The `atlas` CLI maps the codebase (files → functions → who calls what) with no AI involved. Your only job is to write **plain-language explanations** for exactly what it hands you, then save them. They are stored in `.atlas/notes/` (sidecar files that never touch the code) and appear in the local map app (`atlas open`).

**On demand only.** Do nothing unless the user ran `/atlas …` or asked in words. Never explain the whole project unless they say so. Never re-explain code whose explanation is still up to date.

## 0. Tool check (once)

```bash
atlas --help || cargo install --path <RCFH-repo>/tools/atlas
```

If `.atlas/config.json` doesn't exist, ask the user once which language and level they want, then run `atlas init --lang th|en --level beginner|dev`. The default is Thai, beginner.

## 1. Pick the scope from the user's words

| user says | run |
|---|---|
| `/atlas` (nothing else) | `atlas status`, then ask which folder or file. Suggest `--changed` if there are changed files |
| `/atlas src/payments` / a file / `file#function` | `atlas pending src/payments` |
| `/atlas --changed`, "อธิบายโค้ดใหม่", "new code" | `atlas pending --changed` (files changed in git) |
| "อธิบายใหม่ / redo" | add `--force` |
| "ใส่คำอธิบายลงในโค้ด", "write it into the code" | after explaining: `atlas inline <scope>` (dry run), show the result, then `--apply` |
| `/atlas check`, "ตรวจ error", "มีอะไรพังไหม" | see **Errors and debugging** below |
| `/atlas debug <command>`, "รันแล้วพัง", a pasted error / stack trace | see **Errors and debugging** below |
| "ถ้าแก้ X จะกระทบอะไร", "ใครเรียกฟังก์ชันนี้" (what is affected if I change X, who calls this function) | `atlas trace <id or name>` and explain the result |

`pending` returns JSON with:
- `config`: the language and level to write in
- `files[]`: each with `items[]`. Each item has `id`, `kind`, `code`, `calls`, `called_by`, `status` (`missing` | `stale`, plus `old_summary` when stale)
- `file_status` per file, `dirs_without_summary`, and `more_pending`

Exit code 2 means nothing left to explain.

## 2. Write the explanations

Write one JSON array to a temp file such as `.atlas/tmp.json`, then run `atlas write .atlas/tmp.json` and delete the temp file.

```json
[
  {"id": "src/cart.ts#applyCoupon", "summary": "…", "detail": "…"},
  {"file": "src/cart.ts", "summary": "…"},
  {"dir": "src/", "summary": "…"},
  {"dir": "", "summary": "what the whole project is, in one sentence"}
]
```

**How to write for someone who knows 0% code** (level `beginner`):
- **summary** is one sentence about *what it does for the user or business*, not how. Example: "ลดราคาสินค้าในตะกร้าเมื่อลูกค้ากรอกโค้ดส่วนลด" (reduces the cart price when the customer enters a coupon code). Do not write "Applies the coupon object to the cart state".
- **detail** (skip it for trivial code under about 5 lines) is 2–5 short lines:
  - the steps as `1. 2. 3.`, in everyday words
  - what it *receives* and *gives back*, as real-world things ("receives: the customer's coupon code", "gives back: the new total price")
  - an analogy only if it really helps
  - impact, from `called_by`: "ถ้าแก้ตรงนี้ จะกระทบ: หน้าชำระเงิน, ใบเสร็จ" (if you change this, it affects: the checkout page, the receipt)
  - warnings: deletes data, charges money, sends email, needs internet
- No jargon. Don't write *function, parameter, return, async, API, null, array, state*. Say *ขั้นตอน, ข้อมูลที่รับ, ผลลัพธ์, รอคำตอบจากเซิร์ฟเวอร์, รายการ, ค่าว่าง* (step, data it receives, result, waits for the server's answer, list, empty value). If a technical word is unavoidable, explain it in brackets the first time.
- **Be accurate.** Describe only what the `code` shows. If you can't tell (for example, the code calls something outside the scope), say what is visible and stop. Never invent behavior.
- **Keep it short.** Around 60 words or fewer per item. A class or type gets one sentence about what it represents.
- Level `dev`: the same structure, with technical terms allowed, focused on intent, side effects and gotchas.
- For `stale` items, read `old_summary` and update it rather than starting over.
- Write a `file` summary for each file listed, and a `dir` summary for `dirs_without_summary`. Write the project summary (`"dir": ""`) once.

## 3. Loop and finish

If `more_pending` > 0, run `pending` again with the same scope and repeat. Stop when the scope is done, or ask before continuing if it is large (more than about 3 rounds). Then reply in the user's language:
- how many items were explained
- the 3–5 most important things you learned about this part of the code, in plain words
- "เปิดแผนที่ดู: `atlas open`" (open the map: `atlas open`), and the inline option if they haven't used it

## Errors and debugging (only when asked)

The CLI finds the problems; you explain them. Pick one of these to find them:

| situation | run |
|---|---|
| "check the project", nothing is running yet | `atlas check [scope]`, which runs a built-in syntax scan plus the installed checkers (tsc, python, ruff, cargo check, go vet). Add `--tests` only if the user wants the tests run |
| it crashes when they run something | `atlas debug -- <their command>`, e.g. `atlas debug -- npm run build` or `atlas debug -- python app.py`. Never run a command that changes data, deploys, or costs money without asking first |
| they pasted an error | save it to `.atlas/tmp.log`, then run `atlas debug --log .atlas/tmp.log` |

Each problem in the JSON has: `id`, `file`, `line`, `symbol` (the function it is in), `message` (the tool's own words), `snippet` (code around it, `>` marks the line), `affects` (callers that break too), `frames` (the path a runtime error took), and `rerun` (commands that reproduce it). `runs` lists what was checked or skipped and why, for example "TypeScript is not installed — run `npm install`". Pass skipped checks on to the user. If nothing mapped to the code, `output_tail` holds the raw output: explain that instead.

For each problem (the 10 most important, errors before warnings), read the `snippet` and, if needed, `atlas show <symbol>` for what the function is for. Then write:

```json
[{"problem": "<id>",
  "what": "one sentence: what goes wrong, in terms of what the user sees",
  "why":  "the cause in everyday words; translate the error name once, e.g. KeyError = หาช่องข้อมูลชื่อนี้ไม่เจอ (couldn't find a data field with this name)",
  "fix":  "1. … 2. … concrete steps; quote the exact change (\"เปลี่ยน %d เป็น %s\" = change %d to %s); say which fix is the real root cause",
  "try":  ["the command that shows the problem", "the command that confirms it is fixed"]}]
```

Save it with `atlas write .atlas/tmp.json` and delete the temp file.

- `try` holds commands the user can copy and run **themselves**, from the project folder: the reproduce command from `rerun`, and a check after fixing (`atlas check <file>`, the test command, or running the program). Every command must be real and work as written. Never invent flags.
- **Be honest about certainty.** If the cause is a guess, say "น่าจะเป็นเพราะ…" (it's probably because…) and give a command that would confirm it.
- **Don't change code** unless the user asks you to fix it. If they ask: fix, run the `try` commands to verify, then run `atlas check <file>` so the problem disappears from the list.
- Then reply in the user's language: how many problems, the most important one first in 2–3 sentences, and point them to `atlas problems` (terminal) or the **ปัญหา** (Problems) tab in `atlas open`.

## Writing into the source (optional, only when asked)

`atlas inline <scope>` previews and `atlas inline <scope> --apply` writes. The comment blocks are tagged `[atlas]`:
- `/** [atlas] … */` for JS/TS
- `# [atlas]` for Python
- `/// [atlas]` for Rust
- `// [atlas]` for Go

Every file is checked first: its code structure (the AST, without comments) must be identical before and after the change, or the file is skipped. Outdated explanations are never written. `atlas uninline <scope> --apply` removes them all. Tell the user to review the result with `git diff`.

## Never

- Run atlas proactively, after your own code edits, or on a schedule.
- Change code while explaining it. Explanations go only through `atlas write` / `atlas inline`.
- Read whole files to explain them. `pending` already gives you exactly the code you need.
