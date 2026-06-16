# Background-first computer control — acceptance & operator runbook (epic #717)

This is the **manual acceptance runbook** required by #1009 (explicitly *not* an
automated FSV harness) and the human-active acceptance procedure required by
#1001, plus the operator-safe bootstrap for #1011. It records the
Source-of-Truth (SoT) readbacks to inspect for each gate.

Status legend: **VERIFIED** = proven against an independent SoT in this repo;
**OPERATOR** = requires the human at the machine (by design); **CODE-DONE** =
implemented and unit/integration-gated, awaiting the human-active acceptance run.

---

## 1. What the daemon enforces today (verified)

| Invariant | Tool/code | SoT proof |
|---|---|---|
| Default surface hides foreground primitives (#1002/#1008) | `normal_agent` profile (87 tools) | `tool_profile_status` → `denied_break_glass_tools` includes `act_*`; CF_SESSIONS `mcp/tool-profile/v1/<sid>` row |
| Hidden tool fails closed with policy proof (#1002/#1004) | tool-profile policy gate | calling `act_type` from `normal_agent` → error `TOOL_PROFILE_POLICY_DENIED` carrying the CF_SESSIONS `policy_row` |
| Break-glass needs lease + reason + confirm (#999) | `validate_profile_set_policy` | `tool_profile_set break_glass` rejected unless `control_lease` held, `confirm_break_glass=true`, non-empty `reason` |
| Profile change is visible **in-session** (#1020) | `tool_profile_set` → `peer.notify_tool_list_changed()` | `notifications/tools/list_changed` frame on the standalone GET SSE stream; `tools/list` widens 87→150 with **no reconnect** |
| Agent target distinct from human foreground (#994) | `window_list` / `set_target` | `window_list.human_os_foreground_hwnd` reported separately; per-entry `is_foreground`; `GetForegroundWindow()` cross-check matches |
| Passive target discovery without shelling out (#1021) | `window_list` | HWND+PID rows match Win32 `Get-Process MainWindowHandle`; round-trips through `set_target` with no activation |

### Reproduce (isolated daemon, no disruption to the live :7700 daemon)

```sh
# 1. build + launch an isolated daemon on a throwaway port/db
cargo build -p synapse-mcp --bin synapse-mcp
TMPDB=$(mktemp -d)
./target/debug/synapse-mcp.exe --mode http --bind 127.0.0.1:7711 --db "$TMPDB" --log-level info &

# 2. drive it with the bundled FSV client (token from %APPDATA%\synapse\token.txt)
python tmp/fsv_mcp.py        # asserts #1020 + #1021 end-to-end, exit 0 = pass
```

`tmp/fsv_mcp.py` opens the standalone SSE stream, captures the
`notifications/tools/list_changed` frame, flips the profile, and proves the tool
surface widened in the same session. Cross-check `window_list` against Win32:

```powershell
Add-Type 'using System;using System.Runtime.InteropServices;
  public class Fg{ [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow(); }'
[int64][Fg]::GetForegroundWindow()        # == window_list.human_os_foreground_hwnd
Get-Process | ? {$_.MainWindowHandle -ne 0} | % { "{0} {1} {2}" -f [int64]$_.MainWindowHandle,$_.Id,$_.ProcessName }
```

---

## 2. Operator-safe Chrome-bridge bootstrap (#1011, unblocks #996/#997/#1000)

The daemon ships the new bridge extension on disk
(`extensions/synapse-chrome-debugger`, build
`synapse-chrome-bridge-2026-06-15-1011-reload-self-997-type-active-v1`) which
implements `targetInfo`, `typeActiveElement`, `navigateTab`, `openTab`,
`closeTab`, and `reloadSelf`. **The currently *loaded* worker in the running
Chrome predates `reloadSelf`**, so the background self-reload path cannot
activate the new worker — this is the documented chicken-and-egg in #1011.

The daemon already behaves correctly while stale (verified in `health`):
- `chrome_bridge.status = "stale"`, `extension_stale = true`
- `extension_stale_reasons` names the exact missing capabilities and expected
  build id/sha256 (fail-closed, no silent fallback)
- any bridge command requiring a missing capability fails with
  `CHROME_BRIDGE_EXTENSION_STALE` rather than foregrounding.

**One-time operator activation (the only hands-on step; do it once):**
1. Confirm the on-disk build matches the daemon's expected hash:
   `health` → `chrome_bridge.extension_build_sha256 expected=…`.
2. In the already-open Chrome, open `chrome://extensions`, enable Developer
   mode, and click **Reload** on "Synapse Chrome bridge" — OR fully quit and
   reopen Chrome. (This is unavoidable for the *first* activation because the
   loaded worker has no `reloadSelf`; every subsequent update uses
   `cdp_bridge_reload` with no foreground.)
3. Re-read `health`: `chrome_bridge.status` must flip to `ok`,
   `extension_capabilities` must list all six commands, `extension_build_id`
   must equal the expected build.

After activation, `cdp_bridge_reload` performs all future reloads in the
background (`chrome.runtime.reload()`), so this manual step never recurs.

Then verify #996/#997/#1000 (no new Chrome process, human foreground on a
different window throughout):
- **#996**: `cdp_target_info` on an owned background tab returns url/title/
  ready_state/active-element with `backend_tier_used` non-foreground and no
  debugger attach. SoT: Chrome tab table; `GetForegroundWindow` unchanged.
- **#997**: `act_type` into a known `<input>`/`<textarea>`/contenteditable in an
  **inactive** owned tab; read the DOM value back via `cdp_target_info`. SoT:
  target DOM value; OS foreground/cursor unchanged; action-audit tier=background.
- **#1000**: open/bind a dashboard tab, observe an approval inactive, decide it
  from that tab; read CF_KV approval row + mailbox. SoT: CF_KV decision row.

---

## 3. Human-active non-interference acceptance run (#1001 — the final gate)

Run this with **you actively using the computer** (switching foreground
windows, typing, moving the mouse) the entire time.

Setup:
- Start **two** Synapse MCP sessions (two agents / two terminals), each binds a
  **different** background window via `window_list` → `set_target` →
  `target_claim`. Neither claims the window you (the human) are using.
- Keep a third shell open to sample SoT.

During the run, while the human keeps working:
1. Agent A: `cdp_navigate_tab` / `act_type` (break-glass into its *own* tab only)
   / `read_text` / `observe` / `capture_screenshot` on its target.
2. Agent B: `act_run_shell` in its workspace + `observe`/`window_list` on its
   own target.
3. Neither agent calls a human-foreground tier unless explicitly testing the
   break-glass lease path.

SoT samples to capture **before / during / after** (paste into the #1001 close
comment):
- `session_list` — two live sessions, distinct `active target`, distinct lease
  ownership.
- `target_claim_status` — each window owned by exactly one session; no overlap.
- `GetForegroundWindow()` / `GetCursorPos()` — the human's foreground/cursor are
  whatever the human set; **no agent action changed them** (sample repeatedly).
- Per-target DOM/UIA/window readbacks — each agent sees only its own target.
- `CF_ACTION_LOG` rows — `backend tier` is background for all routine actions;
  any foreground tier row is present **only** for the explicit break-glass test
  and carries a held lease id.

**Pass = ** two agents act/read in their own targets with zero cross-talk while
the human freely uses the foreground; the only foreground-tier action in the
audit is the deliberate break-glass test. Close #1001/#755 with this evidence.

---

## 4. Behavioral tool-affordance acceptance (#1009)

Goal: prove the task-scoped surface steers agents to background-safe tools.
Compare these `tools/list` profiles via real spawned agents / the wired client:

| Profile | How to get it | Expected surface |
|---|---|---|
| raw/full legacy | `SYNAPSE_DEBUG_TOOLS=1 SYNAPSE_ENABLE_EVERQUEST=1` | full implementation surface (~177) |
| normal background-safe | default `normal_agent` | 87 tools; no `act_*` foreground primitives; `window_list`/`set_target`/`cdp_*` present |
| browser-control task | `tool_profile_set browser_control` | narrower; perception + cdp + target tools only |
| break-glass/admin | lease + `tool_profile_set break_glass` | full raw surface incl. `act_*` |

For each, give the same synthetic task with a known background solution
("read the title of the LinkedIn tab", "type into the dashboard search box",
"run `git status` in the repo") and record, per the #1009 schema:
- first tool attempted, any rejected tool attempts, selected target semantics,
  whether any foreground-tier tool was attempted.

Expected: normal/browser-control agents pick `window_list`→`set_target`→
`cdp_*`/`read_text`; they never see or attempt `act_*`. Any attempt at a hidden
tool returns `TOOL_PROFILE_POLICY_DENIED` with the CF_SESSIONS policy row (a
physical audit row). Break-glass agents may use `act_*` only after the lease.
Feed the table into the #1007 matrix (`docs/multi-agent-capability-matrix.md`,
already kept in sync by `multi_agent_capability_matrix.rs`).

---

## 5. Remaining code work (tracked, not yet landed)

- **#1006** — enrich `CF_ACTION_LOG` rows with `backend_tier`, `required_foreground`,
  `foreground_policy`, `lease_id`, allowed/denied, and emit a high-severity
  escalation when a `normal_agent` session attempts a foreground tier. (Touches
  the action-audit hot path; coordinate with the in-progress Codex work on that
  file to avoid a shared-tree collision.)
- **#1005** — a high-level background computer-use router (`navigate/click/
  set_field/read/screenshot/run_shell` by target capability) so models pick one
  safe verb instead of raw primitives. Net-new tool surface.
- **#998** — `verify_delta` preflight: read the postcondition surface *before*
  mutation and fail closed if unreadable; covered partly by the bridge
  fail-closed path, full preflight wiring outstanding.
