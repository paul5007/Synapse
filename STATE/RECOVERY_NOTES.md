# RECOVERY NOTES - Synapse

Resume by:
1. Re-read `docs/AICodingAgentSuperPrompt.md`, `AGENTS.md`, #351, open issue queue, and this `STATE/*` directory.
2. Continue #589 first. Code removal is already in local commit `e0e9993`; remaining work is stale systemspec docs, checks, manual FSV, issue comment/close, then push with `[skip ci]`.
3. Clean `docs/systemspec` source files, rerun `docs/systemspec/bundle.ps1`, and re-run `rg` for old live HID surfaces. Retired-link stubs may still mention the removed terms only as historical absence.
4. Run supporting checks (`cargo fmt`, `cargo check -p synapse-mcp`, focused tests, docs check) as regression evidence only.
5. Perform manual #589 FSV with real `synapse-mcp`: process/bind SoT, authenticated health, strict client-parity `tools/list`, real `tools/call` for removed hardware backend behavior, and separate SoT readbacks before/after happy path plus at least 3 edge cases.
6. Amend/commit with `[skip ci]` before any push. Then update/close #589 and continue #590, #585, and #588 context closure.

Do not use GitHub Actions/CI. Do not create FSV scripts or harnesses. For Synapse behavior FSV, prove the real `synapse-mcp` runtime and client-parity tool list before a real tool call, then read the physical SoT separately.
