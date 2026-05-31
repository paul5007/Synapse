# RECOVERY NOTES - Synapse

Resume by:
1. Re-read `docs/AICodingAgentSuperPrompt.md`, `C:\Users\hotra\Downloads\AICodingAgentSuperPrompt.md`, `AGENTS.md`, #351, the open issue queue, and `STATE/*`.
2. #590 is resolved and closed. Evidence comment: https://github.com/ChrisRoyse/Synapse/issues/590#issuecomment-4587000980. Commit: `e7e5b25`.
3. #588 is resolved and closed as the software-only input context issue. Evidence comment: https://github.com/ChrisRoyse/Synapse/issues/588#issuecomment-4587002426.
4. Continue #585 next. Read the a11y/UIA implementation first; the previous issue comment says the desired design is a long-lived dedicated MTA worker that owns `UIAutomation` and keeps `UIElement`s thread-local.

Do not use GitHub Actions/CI. Do not create FSV scripts or harnesses. For Synapse behavior FSV, prove the real `synapse-mcp` runtime and client-parity tool list before a real tool call, then read the physical SoT separately.
