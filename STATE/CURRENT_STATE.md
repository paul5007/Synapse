# CURRENT STATE - Synapse

## 2026-05-31
- Required doctrine loaded from `docs/AICodingAgentSuperPrompt.md` and `AGENTS.md`.
- GitHub issue queue read: open issues are #590, #589, #588, and #585.
- #351 closed decision read: manual FSV only; no FSV scripts/tests/harnesses/CI as acceptance; agent commits pushed must include `[skip ci]`.
- Local `main` is ahead of `origin/main` by one commit: `e0e9993 refactor: retire physical hardware-HID path for software-only input (#588)`.
- That local commit removes the RP2040/Pico firmware, `synapse-hid-host`, hardware action backend, HID CLI, hardware-consent/agreement flow, related fuzz/bench/release artifacts, and updates core code/tests/docs. Its commit message currently lacks `[skip ci]`; amend before pushing.
- Current dirty worktree is documentation-only plus one temp artifact:
  - Modified docs under `docs/computergames/`, `docs/impplan/`.
  - Added `docs/computergames/09_hardware_hid_gateway.md` as a retired-link stub.
  - Untracked `tmp_review.txt` contains a temporary captured diff and should not be committed.
- File-tree SoT reads now return false for `crates/synapse-hid-host`, `firmware/pico-hid`, and `crates/synapse-action/src/backend/hardware`.
- `docs/systemspec/**` still contains stale live hardware-HID references and must be cleaned, then `docs/systemspec/bundle.ps1` rerun.
- #589 has a progress comment saying `firmware/pico-hid` was deleted and the robust plan is to remove the dead HID implementation/operator surface while keeping hardware enum tags routed to a clear unavailable backend error.
- #589 resume comment posted: current SoT still contains `crates/synapse-hid-host`, `firmware/pico-hid`, `--hardware-hid`/`SYNAPSE_HARDWARE_HID`, and health HID status surfaces.

## Open Queue Snapshot
- #588: context/decision, software-only input strategy; physical HID path abandoned.
- #589: remove dead hardware-HID path. Claimed/resumed in issue comment on 2026-05-31.
- #590: add software-backend input fidelity benchmarks for SendInput and ViGEm timing.
- #585: hardening, move UIA calls to a dedicated MTA worker thread; prior comment says this is a larger refactor, not a correctness fix.
