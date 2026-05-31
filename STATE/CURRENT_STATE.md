# CURRENT STATE - Synapse

## 2026-05-31
- Required wake-up context was re-read after compaction:
  - `C:\code\Synapse\docs\AICodingAgentSuperPrompt.md`
  - `C:\Users\hotra\Downloads\AICodingAgentSuperPrompt.md`
  - `AGENTS.md`
  - #351 manual-FSV/no-CI decision
  - open queue and active issue comments
- Open GitHub queue after #590/#588 closure: #585 only.
- #590 was committed/pushed as `e7e5b25`, RESOLVED-commented, and closed.
- #588 was closed as a context issue after #589 and #590 were verified and closed.
- Active work: #585 hardening(a11y), dedicated MTA UIA worker thread.

## #590 Implementation
- Added `crates/synapse-action/benches/action_software_click.rs`.
  - Safe recording-backed default Criterion bench.
  - Opt-in real Windows SendInput path gated by `SYNAPSE_ACTION_SOFTWARE_CLICK_REAL=1`.
  - Local target constant: p99 <= 5 ms for real SendInput.
- Added `crates/synapse-action/benches/action_vigem_pad_report.rs`.
  - Safe recording-backed default Criterion bench.
  - Opt-in real ViGEm/XInput path gated by `SYNAPSE_ACTION_VIGEM_PAD_REAL=1`.
  - Local targets: p99 <= 5 ms and throughput >= 500 reports/s.
- Registered both benches in `crates/synapse-action/Cargo.toml`.
- Updated live docs to retire abandoned physical-HID benches and track SendInput/ViGEm software benches:
  - `docs/impplan/07_cross_cutting.md`
  - `docs/computergames/10_performance_budget.md`
  - `docs/computergames/13_testing_strategy.md`
  - `docs/impplan/00_methodology.md`
  - `docs/computergames/00_vision_and_scope.md`
  - `docs/computergames/14_build_and_packaging.md`
  - `docs/computergames/README.md`
  - `docs/systemspec/14_test_suite.md`
  - `docs/systemspec/SYNAPSE_SYSTEMSPEC.md`
- Added an explicit `#[expect(dead_code)]` on `element_screen_point` because it is reserved for element-target action paths and clippy `-D warnings` would otherwise fail.

## #590 Manual FSV Evidence Captured
- Repo-built daemon:
  - PID `43376`
  - binary `C:\code\Synapse\target\release\synapse-mcp.exe`
  - bind `127.0.0.1:7794`
  - isolated DB `C:\code\Synapse\.runs\590\http-fsv\db2`
  - bearer token from env
- Runtime precondition:
  - process/socket read showed PID `43376` listening on `127.0.0.1:7794`.
  - authenticated `/health` and MCP `health` returned `ok=true`.
  - subsystems were `action,audio,http,profiles,reflex,storage`; no `hid_host`.
  - official MCP Inspector CLI `tools/list` exited 0 with 80 tools.
  - Required tools present: `health`, `storage_inspect`, `act_press`, `act_click`, `act_pad`, `release_all`.
  - No tool names matched `hid|hardware`.
- Happy path SoTs:
  - `act_press` via real MCP `tools/call`: before `CF_ACTION_LOG=0`, Shift up; during hold Shift down; after Shift up; result `ok=true`, `backend_used=software`; after `CF_ACTION_LOG=2` with `act_press started/ok`.
  - `act_click` via real MCP `tools/call`: before `CF_ACTION_LOG=2`, left button up; during hold left button down; after left button up; result `ok=true`, `backend_used=software`, `used_invoke_pattern=false`; after `CF_ACTION_LOG=4` with `act_click started/ok`.
  - `act_pad` via real MCP `tools/call`: before `CF_ACTION_LOG=4`, ViGEm PnP `Nefarius Virtual Gamepad Emulation Bus` `OK`, XInput slots disconnected; after `act_pad`, XInput slot 0 connected with `buttons_hex=0x1000` / A down and `CF_ACTION_LOG=6` with `act_pad started/ok`; after real MCP `release_all`, slot 0 `buttons_hex=0x0000`, `neutralized_pads=1`, `CF_ACTION_LOG=7`.
- Edge cases:
  - Edge 1 `act_press keys=[]`: before `CF_ACTION_LOG=7`; trigger exited 1 with `act_press keys must contain at least one key`; after `CF_ACTION_LOG=9` with `TOOL_PARAMS_INVALID`.
  - Edge 2 `act_click clicks=0`: before `CF_ACTION_LOG=9` and left button up; trigger exited 1 with `act_click clicks must be in 1..=3, got 0`; after `CF_ACTION_LOG=11` with `TOOL_PARAMS_INVALID` and left button still up.
  - Edge 3 `act_pad thumb_l=[1.5,0]`: before `CF_ACTION_LOG=11`, XInput slot 0 neutral; trigger exited 1 with invalid axis message; after `CF_ACTION_LOG=13` with `TOOL_PARAMS_INVALID` and XInput unchanged neutral.
  - Edge 4 `act_pad backend=hardware`: before `CF_ACTION_LOG=13`; trigger exited 1 with removed-hardware backend message; after `CF_ACTION_LOG=15` with `ACTION_BACKEND_UNAVAILABLE` and XInput unchanged neutral.
- Cleanup:
  - final real MCP `release_all` returned `released_keys=0`, `released_buttons=0`, `neutralized_pads=0`.
  - final OS read: Shift up and left button up.
  - final post-bench XInput read: slot 0 connected neutral, slots 1-3 disconnected.
  - repo-built FSV daemon PID `43376` was stopped; process/socket read showed no PID and no listener on port `7794`.

## Supporting Checks
- `cargo bench -p synapse-action --bench action_software_click`
  - recording p50 `27300 ns`, p99 `63800 ns`, max `233700 ns`, pass.
  - real SendInput bench intentionally skipped because `SYNAPSE_ACTION_SOFTWARE_CLICK_REAL` was unset; real SendInput was manually FSV-verified through MCP.
- `SYNAPSE_ACTION_VIGEM_PAD_REAL=1 cargo bench -p synapse-action --bench action_vigem_pad_report`
  - recording p50 `41400 ns`, p99 `138200 ns`, max `248900 ns`, `650 reports/s`, pass.
  - real ViGEm p50 `83700 ns`, p99 `284500 ns`, max `4989300 ns`, `638 reports/s`, pass against 5 ms / 500 reports/s target.
- `cargo fmt --check`
- `cargo clippy -p synapse-action --bench action_software_click --bench action_vigem_pad_report -- -D warnings`
- `cargo check -p synapse-mcp`
- `pwsh scripts\check_docs.ps1`
- `git diff --check` returned only line-ending warnings.
- No GitHub Actions/CI were run or used.

## #590 / #588 GitHub Closure
- #590 RESOLVED evidence comment: https://github.com/ChrisRoyse/Synapse/issues/590#issuecomment-4587000980
- #590 closed at `2026-05-31T14:26:26Z`.
- #588 context RESOLVED comment: https://github.com/ChrisRoyse/Synapse/issues/588#issuecomment-4587002426
- #588 closed after #589 and #590 readback showed both concrete follow-ups closed.

## Next
- All GitHub issues are closed as of the live queue read after #585 closure.
- Final verification before ending the objective: `gh issue list --repo ChrisRoyse/Synapse --state open --limit 100` returned no rows; #585 readback is `CLOSED`; `HEAD == origin/main == 0814a41` for the #585 implementation commit.

## #585 Implementation In Working Tree
- `synapse-a11y` now initializes one long-lived `synapse-a11y-uia-mta` worker thread on first use.
  - The worker calls `CoInitializeEx(COINIT_MULTITHREADED)`, owns the `UIAutomation` object, and processes a channel of `FnOnce(&UIAutomation) -> A11yResult<T>` jobs where `T: Send`.
  - `uia_worker_readback()` reads the worker thread id, COM apartment, and owned window count from the worker itself.
  - Initialization is protected by a `OnceLock` plus mutex so concurrent first calls do not race.
- Direct `UIElement` APIs on Windows now fail closed with structured internal errors and point callers to data-returning APIs.
- New data-returning APIs keep COM pointers thread-local:
  - `snapshot_focused_window`, `snapshot_window_from_hwnd`, `snapshot_window_for_process`, `snapshot_element`
  - `focused_element_node`, `element_node_from_point`
  - `find_by_name_and_pattern_in_window`, `top_level_window_hwnd_by_name`
  - `element_bounding_rect`, `click_element_action`, `focus_element`, `expand_state_of_id`
- Runtime call sites were migrated:
  - MCP `observe` uses `snapshot_focused_window`.
  - HUD text fallback uses `element_node_from_point`.
  - M2 drag and action invoke use data-returning bbox/invoke helpers.
  - Notepad/perception/a11y regression tests use the data APIs.
- Snapshot cache now includes the root `ElementId` as well as depth to avoid a fresh-depth cache hit from the wrong window.
- Docs were updated so live impplan/systemspec references describe worker-owned data APIs rather than direct `UIElement` handoff.

## #585 Supporting Checks Completed
- `cargo fmt --check`
- `cargo check -p synapse-a11y`
- `cargo check -p synapse-action`
- `cargo check -p synapse-mcp`
- `cargo check -p synapse-perception --tests`
- `cargo test -p synapse-a11y --lib -- --nocapture`
- `cargo test -p synapse-a11y --test uwp_snapshot_fsv -- --ignored --nocapture`
- `cargo clippy -p synapse-a11y -p synapse-action --all-targets -- -D warnings`
- `cargo clippy -p synapse-mcp --all-targets -- -D warnings`
- `cargo test -p synapse-action --lib invoke -- --nocapture`
- `cargo test -p synapse-mcp --test m2_notepad_type_save --no-run`
- `pwsh scripts\check_docs.ps1`
- `git diff --check`
- `cargo build --release -p synapse-mcp`

## #585 Manual FSV Evidence Captured
- Repo-built daemon:
  - PID `43940`
  - binary `C:\code\Synapse\target\release\synapse-mcp.exe`
  - bind `127.0.0.1:7795`
  - isolated DB `C:\code\Synapse\.runs\585\http-fsv-20260531T1001\db`
  - logs and captured tool outputs under `C:\code\Synapse\.runs\585\http-fsv-20260531T1001`
- Runtime precondition:
  - process/socket read showed PID `43940` from the repo release binary listening on `127.0.0.1:7795`.
  - authenticated `/health` with the configured `%APPDATA%\synapse\token.txt` token returned `ok=true`.
  - official MCP Inspector CLI `tools/list` exited 0, loaded 80 tools, and included `health`, `observe`, `storage_inspect`, `act_click`, and `act_drag`.
  - real MCP `health` tool call returned `ok=true`.
- Worker SoT:
  - daemon log line: `A11Y_UIA_WORKER_READY thread_id=45584 apartment=Mta owned_window_count=0`.
  - separate OS read after observe load: process thread `45584` still existed in PID `43940`; `EnumThreadWindows(45584)` returned `0`.
  - only one `A11Y_UIA_WORKER_READY` line appeared after concurrent load.
- Happy path:
  - foreground was manually set to Calculator HWND `0xc0bc8`.
  - before storage: `CF_OBSERVATIONS=4`, `CF_EVENTS=4`, `CF_ACTION_LOG=10`.
  - trigger: real MCP `observe` with `depth=6`, `max_elements=500`, `include=["focused","elements","diagnostics"]`.
  - after storage: `CF_OBSERVATIONS=5`, `CF_EVENTS=5`; stored observation row contained `ApplicationFrameHost.exe`, `window_title:"Calculator"`, `profile_id:"calculator"`, 89 elements, and `CalculatorResults` / `Display is 0`.
- Edge 1, depth boundary:
  - before: `CF_OBSERVATIONS=5`, `CF_EVENTS=5`.
  - trigger: real MCP `observe depth=0`.
  - after: `CF_OBSERVATIONS=6`, `CF_EVENTS=6`; stored row contained Calculator and exactly one root element.
- Edge 2, over-large depth/node request:
  - before: `CF_OBSERVATIONS=6`, `CF_EVENTS=6`.
  - trigger: real MCP `observe depth=999 max_elements=9999`; service clamped to documented limits.
  - after: `CF_OBSERVATIONS=7`, `CF_EVENTS=7`; stored row contained Calculator, 89 elements, and `CalculatorResults`.
- Edge 3, structurally invalid params:
  - before: `CF_OBSERVATIONS=7`, `CF_EVENTS=7`, `CF_ACTION_LOG=10`.
  - trigger: Inspector real MCP `observe depth=bad`.
  - result: exit 1, `failed to deserialize parameters: invalid type: string "bad", expected u32`.
  - after: `CF_OBSERVATIONS=7`, `CF_EVENTS=7`, `CF_ACTION_LOG=10`; daemon health remained `ok=true`.
- Edge 4, concurrent observe load:
  - before: `CF_OBSERVATIONS=7`, `CF_EVENTS=7`.
  - trigger: four explicit concurrent official Inspector `observe depth=6 max_elements=500` calls.
  - each returned `ApplicationFrameHost.exe`, HWND `789448`, 89 elements, and `CalculatorResults`.
  - after: `CF_OBSERVATIONS=11`, `CF_EVENTS=11`; worker thread readback stayed one MTA/windowless thread.
- Cleanup:
  - real MCP `release_all` returned `released_keys=0`, `released_buttons=0`, `neutralized_pads=0`.
  - final real MCP `health` returned `ok=true`.
  - repo-built daemon PID `43940` was stopped; process/socket read showed no PID and no listener on `127.0.0.1:7795`.

## #585 GitHub Closure
- #585 RESOLVED evidence comment: https://github.com/ChrisRoyse/Synapse/issues/585#issuecomment-4587147620
- #585 closed at `2026-05-31T15:25:48Z`.
- Implementation commit pushed as `0814a41` with `[skip ci]`.
- Live open queue read after closure returned no open issues.
