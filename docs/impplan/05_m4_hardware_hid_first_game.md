# 05 — M4: Hardware HID + First Game Profile (2-3 weeks) — ACTIVE

> M3 closed 2026-05-25 (`v0.1.0-m3` @ `97019ec`); M4 is the active phase as
> of HEAD `6ed52e4`. This file is structured as a planning sketch; the first
> M4 task is a full self-contained M2/M3-style rewrite once Block A.0 (M3
> carry-over) lands. **All global invariants apply** (no backcompat, no mocks
> gate completion, FSV with source-of-truth read-back, Natural-only motion,
> manual configured-host shipping gate).

PRD: `docs/computergames/15_roadmap_and_milestones.md` §6. Hardware: `09_hardware_hid_gateway.md`. Firmware: `09 §4`. Wire protocol: `09 §5`. Supported-use policy: `08_*.md`. Doctrine: `00_methodology.md` + `07_cross_cutting.md`.

## Mission (Occam's razor)

**Bring the RP2040 firmware online, fill out `synapse-hid-host` so `Backend::Hardware` routes to a real device over CDC ACM, and ship the first game profile (`minecraft.java`) with HUD template-match + keymap + supported-use gates.** Every other M4 clause is a consequence of that sentence plus the global invariants.

## Goal

RP2040 firmware (Rust + `embassy-rp`) + serial driver (`synapse-hid-host`) + `act_combo` MCP tool (the M3 reflex scheduler is reused — `act_combo` compiles to a `combo` reflex). First game profile `minecraft.java` with HUD extractors + keymap. Profile supported-use gates enforced.

## Demo gate

**Primary:** Agent connects to Synapse with Minecraft Java running. Calls `observe()` → sees HP hearts + visible entities. Walks "find tree → break tree → make planks → make workbench" via `act_press` / `act_aim` + 1-2 reflexes (e.g., `auto_attack_low_hp` ⇒ `on_event hud_value_changed field=hp new<8 → key_press("e")`). Runs 5 min hands-off.

**Bonus:** Same demo via `--hardware-hid auto` (RP2040 flashed + plugged).

---

## Inputs

- M3 demo gate passed; `v0.1.0-m3` tag cut on commit `97019ec` (2026-05-25)
- 30 MCP tools live on `main` (6 M1 + 9 M2 + 15 M3); reflex scheduler + RocksDB + WASAPI + HTTP/SSE + 4 bundled profiles operational; operator panic hotkey (`Ctrl+Alt+Shift+P`) wired
- Hardware: 1× Raspberry Pi Pico (RP2040), USB-A cable, host PC with free USB port
- Rust toolchain extension: `rustup target add thumbv6m-none-eabi`; `cargo install elf2uf2-rs`
- Minecraft Java Edition installed (single-player creative/survival world for testing)
- `embassy-rp` + `embassy-usb` resolvable; `serialport = "4.9.0"` already in `[workspace.dependencies]` at the repo root
- `firmware/pico-hid/` directory does **not yet exist** (excluded from workspace per root `Cargo.toml:21`). M4 work-item 1 creates it from scratch with its own `Cargo.toml` (separate workspace targeting `thumbv6m-none-eabi`).
- `synapse-hid-host` crate currently empty stub (1 LoC `lib.rs`); M4 fills it out.
- `Backend::Hardware` already routes through `synapse-action`'s `HardwareUnavailableBackend` returning `ACTION_BACKEND_UNAVAILABLE`; M4 replaces that route with a real `HardwareBackend` impl that proxies to `synapse-hid-host`.
- M3 carry-over (see `04_m3_reflex_mcp_surface.md` header): Block A.0 splits the over-cap files (synapse-a11y/lib.rs 2087, synapse-capture/lib.rs 1798, synapse-core/types.rs 1567, synapse-mcp/server.rs 1335, synapse-mcp/m3/reflex.rs 1165, synapse-reflex/lib.rs 986, synapse-reflex/scheduler.rs 890, synapse-mcp/http/sse.rs 764, synapse-mcp/m3/replay.rs 651) **before** building hardware HID on top, and fixes the CHANGELOG M3 tool-name drift (`profile_get`→`profile_list`, `profile_set_active`→`profile_activate`, and adds the four `storage_*` diagnostic tools to the entry).

---

## Deliverables

### Firmware (`firmware/pico-hid/`)

Per `09 §4`:

```
firmware/pico-hid/
├── Cargo.toml                  (separate workspace; target thumbv6m-none-eabi)
├── memory.x
├── build.rs                    (elf2uf2 step)
├── src/
│   ├── main.rs                 (embassy executor; spawns device/serial/dispatch/watchdog/led)
│   ├── usb.rs                  (composite descriptor builder)
│   ├── hid_descriptors.rs      (mouse boot+ext / kbd boot / xinput-like pad)
│   ├── reports.rs              (report structs)
│   ├── serial.rs               (CDC ACM)
│   ├── protocol.rs             (frame parser: MAGIC=0x5A, LEN u16, SEQ u32, CMD u8, payload, CRC16/CCITT-FALSE)
│   ├── pad_state.rs            (14-byte XInput-like report accumulator)
│   ├── safety.rs               (watchdog default 1000 ms ⇒ RELEASE_ALL internal)
│   └── led.rs                  (idle slow blink / active steady / watchdog fast / error SOS)
└── tests/protocol_roundtrip.rs (host-side parser; runs as a local supporting check)
```

### Host driver (`synapse-hid-host`)

Per `09 §7`:

- `HidGateway::connect(port_name)` with 1 Mbaud serial (informational), 5 ms read timeout
- Identity handshake via `IDENTIFY` cmd; reject if `fw_ver.major != EXPECTED_FW_MAJOR` ⇒ `HID_FIRMWARE_VERSION_MISMATCH`
- Pipelined send: up to 16 outstanding unacked frames; ACK ≤ 5 ms or retry up to 3× then `HID_LINK_TIMEOUT`
- Auto-detect via `--hardware-hid auto` enumerating COM ports + sending `IDENTIFY`
- Reconnect every 500 ms on serial error; while disconnected, `Backend::Hardware` calls fail fast w/ `ACTION_HID_PORT_DISCONNECTED`

### Action backend extension

`synapse-action` adds `HardwareBackend` routing per `03 §9`:

- `MouseMoveRelative` ⇒ `MOUSE_MOVE_REL [i16 dx][i16 dy]`
- `KeyPress` ⇒ `KEY_DOWN [u8 hid_code]` + sleep + `KEY_UP`
- `MouseButton` ⇒ `MOUSE_BUTTON [u8 button][u8 down_flag]`
- `PadReport` ⇒ `PAD_REPORT [14 raw bytes]`
- `Action::ReleaseAll` ⇒ firmware `RELEASE_ALL` (0x40)

### Action combo via reflex scheduler

`act_combo` MCP tool (`05 §3.18`) compiles to a reflex of kind `combo`. Same scheduler thread fires steps at exact `at_ms` offsets. Backend route per call.

### `synapse-mcp` adds tools

| Tool | PRD |
|---|---|
| `act_combo` | `05 §3.18` |
| `act_run_shell` | `05 §3.20` (gated; `--allow-shell <regex>` required) |
| `act_launch` | `05 §3.21` (gated; `--allow-launch <regex>` required) |

### Sub-commands (CLI extensions)

| Command | Effect |
|---|---|
| `synapse-mcp hid identify --port COM7` | sends IDENTIFY, prints `IDENTIFY_RESP` |
| `synapse-mcp hid flash --port COM7` | resets to bootloader, copies bundled `.uf2`, re-verifies |

### Profile: `profiles/minecraft.java.toml`

Per `07 §8.2` (full example included there). Key fields:

- `mode = "pixel_only"`
- `[[matches]]` `exe = "javaw.exe"`, `title_regex = "Minecraft.*[0-9]"`
- `[detection]` `model_id = "yolov10n_general"` (note: `OQ-025` — operator imports weights; AGPL Ultralytics weights not bundled), `classes_of_interest = ["player","zombie","skeleton","creeper","villager"]`
- `[[hud]]` `hp_hearts` (template match), `hunger`, `xp` (regions anchored to `bottom_left`/`bottom_right`)
- `[keymap]` `attack=lmb`, `place=rmb`, `inventory=e`, `forward=w`, etc.
- `[[event_extensions]]` `creeper_nearby` (filter: kind=entity-appeared AND class=creeper AND bbox.w>80 ⇒ emit `creeper-imminent`)
- `use_scope = "single_player"`
- `mouse_curve_default = "natural"` + `keyboard_dynamics_default = "natural"` per OQ-004 DECIDED. Aim style `Snap` (50 ms) for menu clicks; combat aim uses reflex `aim_track` w/ Natural per-tick deltas (gain tuned, EMA smoothing α=0.7 per OQ-013); no `Instant` curves in any keymap or HUD action

### HUD asset bundle (`profiles/assets/minecraft.java/`)

```
hearts/full.png, hearts/half.png, hearts/empty.png
hunger/full.png, hunger/half.png, hunger/empty.png
```

Template-match extractor in `synapse-perception::hud` (`02 §5 hud section`).

### Supported-use policy enforcement

Per `08` §3 + §6:

- `Profile.use_scope` field (new): `productivity` | `single_player` | `operator_owned_test` | `sanctioned_research` | `unknown`
- MCP layer checks profile scope + session permissions + backend availability before dispatching `Action`
- `use_scope = "unknown"` refuses write/action tools with `SAFETY_PROFILE_ACTION_DENIED` until an operator activates a reviewed profile or explicit override
- Hardware HID requires `--hardware-hid <port|auto>` plus first-use operator confirmation when configured interactively

### Error codes (must throw + test)

```
HID_PORT_NOT_FOUND
HID_PORT_OPEN_FAILED
HID_PROTOCOL_HANDSHAKE_FAILED
HID_FIRMWARE_VERSION_MISMATCH
HID_COMMAND_REJECTED
HID_LINK_TIMEOUT
ACTION_HID_PORT_DISCONNECTED
SAFETY_PROFILE_ACTION_DENIED
SAFETY_LAUNCH_DENIED_BY_POLICY
SAFETY_SHELL_DENIED_BY_POLICY
SAFETY_OPERATOR_HOTKEY_FIRED
```

---

## Work-items (PR-sized, ordered)

### Block A — firmware (work-items 1-7)

| # | Title | Acceptance |
|---|---|---|
| 1 | `feat(firmware): cargo project + memory.x + embassy-rp init + LED hello-world` | flash to Pico, LED blinks per `09 §9` idle pattern |
| 2 | `feat(firmware): USB CDC ACM serial channel` | host sees COM port; loopback echo test works (10k bytes round-trip lossless) |
| 3 | `feat(firmware): HID composite descriptor (mouse boot+ext / kbd boot / pad XInput-like)` | Windows enumerates all 3 interfaces; `devmgmt.msc` shows HID-compliant devices |
| 4 | `feat(firmware): protocol parser (MAGIC, LEN, SEQ, CMD, payload, CRC) + ACK/NAK` | `tests/protocol_roundtrip.rs` x86 host-parser tests pass; sample frame → cmd dispatch table |
| 5 | `feat(firmware): command dispatcher (MOUSE_*, KEY_*, PAD_REPORT, RELEASE_ALL, WATCHDOG_KICK, IDENTIFY, GET_TELEMETRY)` | host-tested commands all yield matching HID reports on the wire |
| 6 | `feat(firmware): watchdog default 1000 ms ⇒ RELEASE_ALL + telemetry counter` | stop sending for 1.2 s ⇒ telemetry shows watchdog fires + all inputs released |
| 7 | `feat(firmware): elf2uf2 build + uf2 in scripts/release/firmware/` | `cargo build --release --target thumbv6m-none-eabi && elf2uf2-rs` yields `pico-hid.uf2` |

### Block B — host driver (work-items 8-12)

| # | Title | Acceptance |
|---|---|---|
| 8 | `feat(hid-host): serial open w/ identify handshake + fw version check` | mismatch ⇒ `HID_FIRMWARE_VERSION_MISMATCH`; matching ⇒ connected |
| 9 | `feat(hid-host): send pipeline (up to 16 outstanding, 5 ms ACK timeout, 3 retries)` | bench: 1000 mouse-move-rel commands ≤ 1.5 s wall total; 0 drops |
| 10 | `feat(hid-host): auto-detect via --hardware-hid auto enumeration` | one Pico plugged ⇒ found; none ⇒ surface clear error |
| 11 | `feat(hid-host): reconnect loop on serial disconnect` | unplug mid-stream ⇒ subsequent `Backend::Hardware` calls fail w/ `ACTION_HID_PORT_DISCONNECTED`; replug ⇒ auto-resume within 1 s |
| 12 | `feat(action): HardwareBackend routing for all relevant Action variants` | E2E: `act_press(keys=["w"], hold_ms=100, backend="hardware")` produces real keypress observable by external test harness |

### Block C — combo + gated tools (work-items 13-15)

| # | Title | Acceptance |
|---|---|---|
| 13 | `feat(mcp): act_combo compiles to combo reflex; backend route per call` | E2E: 3-step combo via hardware; step intervals within 0.5 ms of scheduled (10 §11 / 13 §9) |
| 14 | `feat(mcp): act_run_shell w/ --allow-shell regex allowlist (11 §4.4)` | unmatched pattern ⇒ `SAFETY_SHELL_DENIED_BY_POLICY`; allowed runs to completion; broad pattern (`.*`) rejected at startup |
| 15 | `feat(mcp): act_launch w/ --allow-launch regex allowlist + wait_for_window_title_regex` | launch Notepad, wait for `Untitled - Notepad`; returns pid + hwnd |

### Block D — Minecraft profile (work-items 16-19)

| # | Title | Acceptance |
|---|---|---|
| 16 | `feat(perception): hud template-match extractor (03 §5 HUD extraction)` | given full/half/empty heart templates, returns count 0..20 with confidence ≥ 0.85 across test frames |
| 17 | `feat(profiles): minecraft.java.toml + hearts/hunger/xp HUD specs + keymap + event_extensions` | profile loads, matches `javaw.exe` Minecraft window, HUD readings populate on real game frames |
| 18 | `feat(perception): event_extensions evaluator (filter ⇒ emit_kind)` | `creeper_nearby` filter test: synthetic entity-appeared event w/ class=creeper bbox.w=100 ⇒ `creeper-imminent` emitted |
| 19 | `test(e2e): minecraft_5min` (manual-gated by maintainer w/ Minecraft running) | 5 min run completes the demo scenario hands-off |

### Block E — supported-use gates (work-items 20-21)

| # | Title | Acceptance |
|---|---|---|
| 20 | `feat(core): Profile.use_scope field + scope-aware action gating in MCP` | `unknown` scope + write/action tool ⇒ `SAFETY_PROFILE_ACTION_DENIED`; `single_player` Minecraft profile unaffected |
| 21 | `feat(safety): hardware HID explicit enablement + interactive prompt` | missing hardware enablement ⇒ refused with specific gate named; enabled path proceeds + acknowledgment recorded in `%APPDATA%\synapse\agreement.json` |

### Block F — bench + release (work-items 22-23)

| # | Title | Acceptance |
|---|---|---|
| 22 | `bench: action_hardware_press p99 ≤ 5 ms (10 §2, requires HW attached)` | criterion bench passes on the configured host with Pico attached; manual evidence records exported metrics |
| 23 | `chore(release): bundled pico-hid-x.y.z.uf2 release asset + hid flash subcommand` | `synapse-mcp hid flash --port COM7` reflashes existing Synapse-firmware Pico end-to-end |

---

## Acceptance gates (block M5)

```
✓ Minecraft demo passes (5 min hands-off via software backend)
✓ Hardware HID demo passes (same scenario w/ --hardware-hid auto)
✓ Bench action_hardware_press p99 ≤ 5 ms (10 §12)
✓ Bench combo step interval within 0.5 ms (13 §9 hid_combo_timing)
✓ Firmware watchdog fires within 1 s of host stop; release_all observed
✓ Hardware HID: refused without explicit enablement; works with configured port/auto-detect and acknowledgment
✓ All M4 error codes throwable + tested
✓ HID protocol roundtrip fuzz: 10 min/target no crashes (13 §11)
✓ Reflash via `hid flash` end-to-end on a Pico
✓ Hardware-in-loop bench `hid_high_volume`: 10k mouse moves no drops (13 §9)
✓ No silent fall-through: `ACTION_HID_PORT_DISCONNECTED` always surfaces when backend=hardware and port down
```

---

## Risks (`15 §9` + extras)

| Risk | Mitigation |
|---|---|
| RP2040 firmware bugs are hard to debug | `--features loopback` firmware build echoes commands as PONG for off-target test; tests/protocol_roundtrip.rs runs as a local supporting check |
| Minecraft detection accuracy weak (Ultralytics weights AGPL — `OQ-025`) | Use any permissively-licensed substitute (RT-DETR-s or community fine-tune); document accuracy lower than `15 §6`; fine-tune planned for v1.x |
| HUD OCR/template-match flakes on varied lighting | Test set across day/night/biome; threshold tuning per profile via `confidence_threshold`; fallback to WinRT OCR + regex parser |
| Hardware HID latency under sustained load | Pipeline depth = 16 outstanding; firmware buffer 64; coalescing per `OQ-016` on hardware backend for sub-2 ms pending small moves |
| ViGEm + hardware HID interplay | Backend selection explicit per call; default per profile; no auto-fallback between virtual ↔ hardware (would mask profile-permission changes) |
| Profile scope changes mid-session | Profile detector re-evaluates `use_scope`; moving into `unknown` pauses write/action emission until the operator activates a reviewed profile |
| `OQ-013` aim_track smoothing under detection jitter | EMA `alpha = 0.7` default; configurable per reflex params; tune from Minecraft gameplay footage |

---

## Out of scope at M4 (deferred ≥ M5)

- Multiple game profiles (1 lighthouse here; 5+ at M5)
- VLM `describe` (Florence-2-base; M5)
- Debug overlay (M5)
- Installer / MSI (M5)
- Per-game fine-tuned detection model (v1.x)
- PIO USB host (v2; `09 §12`)
- Steam Audio HRTF (v2; `OQ-021`)

---

## Definition of Done

M4 closed when:

1. Both demos (primary software-backend Minecraft 5-min hands-off + bonus `--hardware-hid auto` RP2040) pass on a real Win11 box.
2. Every acceptance gate above green; **manual FSV with source-of-truth read-back on every row**.
3. Manual happy-path + edge-case test plan filled in by operator in the M4 release PR (write the plan as the first task once M4 starts; mirror the M3 §8 structure).
4. `CHANGELOG.md` updated; `git tag v0.1.0-m4` cut; bundled `pico-hid-x.y.z.uf2` published as part of the tag's release assets.

**FSV reminder:** every Minecraft manual row records a separate source-of-truth read (`fs::read_to_string` for the saved-world state where possible; UIA `ValuePattern` on the F3 debug screen for coords; `XInputGetState` for the gamepad path; `RecordingBackend` events for the software-backend path; external `WH_KEYBOARD_LL` hook for the hardware-backend path). No row is "ok by inspection" — issue evidence must show `before`, `after`, the separate read, and final observed result for every primary path, with >=3 edge cases each.

Open next: `06_m5_production_polish.md`.
