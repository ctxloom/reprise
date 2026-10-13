---
name: "recover"
description: "recover"
---


Recover the context that `/clear` wiped from THIS session. `/clear` empties the
context window but does not end the session — the work you are recovering still
lives in the current session's still-growing transcript.

## Steps

1. Use the ctxloom MCP tool "recover_session" to retrieve the current session:
   - ctxloom resolves the current (still-live) session at read time and
     re-distills its still-growing transcript. No process tracking is involved.
   - Returns the distilled essence of the current session.

2. If successful (content is returned), review the restored context and summarize:
   - What was being worked on
   - Key decisions made
   - Progress achieved
   - Any planned next steps

3. Ask: "I've recovered context from before the clear. Ready to continue?"

If nothing is recovered:
"No recoverable context found for this session."
