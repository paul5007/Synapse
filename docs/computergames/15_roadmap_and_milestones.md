# 15 — Roadmap and Milestones

## 1. Milestone overview

| Milestone | Theme | Effort (solo) |
|---|---|---|
| **M0** | Bootstrap — workspace, MCP loopback, local checks | 1 week |
| **M1** | Perception MVP — capture + UIA + observe() | 2-3 weeks |
| **M2** | Action MVP — kbd/mouse/pad + clipboard | 2 weeks |
| **M3** | Reflex + MCP surface — tools, push events, profiles | 2-3 weeks |
| **M4** | First game profile + compound actions; hardware path retired | shipped/retired |
| **M5** | Production polish + profile-registry/audit-data moat | 3-4 weeks |

~14 weeks solo full-time to v1.0; ~8 weeks with two engineers. Each milestone has a hard demo criterion. No demo, no milestone.

---

## 2. M0 — Bootstrap (1 week)

**Goal:** empty repo to "MCP server returning hardcoded data."

### Scope

- Cargo workspace, 15 crates (most stubs)
- `synapse-core` types (`Backend`, `Point`, `Rect`, M0 error codes)
- `synapse-mcp` binary with `rmcp`
- One tool: `health` returns `{"ok": true, "version": "..."}`
- stdio transport with Claude Desktop / Codex CLI
- `tracing` JSON file logger
- Local supporting checks: `cargo fmt`, `cargo clippy`, `cargo test`
- README "Hello, Synapse"
- `synapse-test-utils` with custom MCP client

### Out of scope

Real perception/action, storage (`()` placeholder), profiles, models.

### Demo criterion

Claude Desktop calls `health` via Synapse MCP, sees `{"ok": true}`.

### Files created

```
Cargo.toml, deny.toml, .gitignore
LICENSE-MIT, LICENSE-APACHE
README.md
docs/                                  (this PRD)
crates/synapse-core/
crates/synapse-mcp/
crates/synapse-test-utils/
crates/synapse-storage/                (stub)
crates/synapse-perception/             (stub)
crates/synapse-action/                 (stub)
crates/synapse-reflex/                 (stub)
crates/synapse-capture/                (stub)
crates/synapse-a11y/                   (stub)
crates/synapse-audio/                  (stub)
crates/synapse-profiles/               (stub)
crates/synapse-models/                 (stub)
crates/synapse-telemetry/
crates/synapse-overlay/                (stub)
.github/workflows/ci.yml
scripts/release/ (skeleton)
```

---

## 3. M1 — Perception MVP (2-3 weeks)

**Goal:** describe any focused window as structured JSON.

### Scope

- `synapse-capture`: `windows-capture`; emit `CapturedFrame` over crossbeam channel
- `synapse-a11y`: UIA tree walker (depth-limited snapshot); WinEvent hook (foreground, focus, value, structure); small UIA cache (focused element)
- `synapse-perception`: stub detection (empty unless model loaded); WinRT OCR wrapper; `Observation` assembler
- `synapse-models`: minimum ONNX loader; default detector registry now `rtdetr_v2_s_coco_onnx` per ADR-0010
- `synapse-mcp` adds: `observe`, `find`, `read_text`, `set_capture_target`, `set_perception_mode`, `health`
- Coordinate transforms (per-monitor DPI)

### Out of scope

Audio, HUD profiles, reflexes, action, replay log.

### Demo criterion

Notepad focused. `observe()` returns `foreground.process_name = "notepad.exe"`, `focused.role = "Edit"`, editor bounding rect. Round trip ≤ 50 ms.

### Risk areas

- UIA cross-process COM marshaling slow; cache request batching day one
- DirectX texture lifetime; time on `Drop`/`RAII` correctness
- `ort` + DirectML setup on clean Windows has paperwork (MSVC redist)

---

## 4. M2 — Action MVP (2 weeks)

**Goal:** drive any app's input.

### Scope

- `synapse-action`: software backend via `enigo` + direct `windows-rs`; action serialization actor (single mpsc emitter); `ReleaseAll` safety on shutdown/panic; held-input tracking + auto-release timeout; ViGEm backend via `vigem-client`; coordinate transforms for screen/window/element clicks; UIA `InvokePattern` semantic invoke
- `synapse-mcp` adds: `act_click`, `act_type`, `act_press`, `act_aim`, `act_drag`, `act_scroll`, `act_pad`, `act_clipboard`, `release_all`
- Aim curves: `Instant`, `Linear`, `EaseInOut`, `Bezier`, `Natural`
- Keystroke dynamics: `Burst`, `Linear`, `Natural`

### Out of scope

Hardware HID (later retired by #588/#589), combos (M3), run-shell/launch.

### Demo criterion

`act_click(element_id=<Notepad editor>)`, `act_type("Hello")`, `act_press(["ctrl","s"])`; save dialog appears; `observe()` shows it.

### Risk areas

- ViGEm needs ViGEmBus installed on the operator's configured Windows host;
  M2 ships from manual FSV on that host, not from CI runner coverage
- `Natural` curve takes iteration; default `EaseInOut` until M5

---

## 5. M3 — Reflex + MCP Surface (2-3 weeks)

**Goal:** push-event subscriptions, reflexes, profiles, full tool surface.

### Scope

- `synapse-reflex`: event bus (`crossbeam` broadcast); reflex scheduler on dedicated time-critical thread; five reflex kinds (`aim_track`, `hold_move`, `hold_button`, `combo`, `on_event`); audit log to `CF_REFLEX_AUDIT`
- `synapse-storage`: RocksDB integration; CF set per `07_storage_and_profiles.md`; compaction filters for TTL; GC with soft/hard caps; disk pressure responder
- `synapse-profiles`: TOML loader; hot-reload via `notify`; detection (exe + title match); bundled `notepad`, `vscode`, `chrome`, `terminal`
- `synapse-mcp` adds: `subscribe`, `subscribe_cancel`, `reflex_register`, `reflex_cancel`, `reflex_list`, `reflex_history`, `profile_list`, `profile_activate`, `replay_record`, `audio_tail`, `audio_transcribe`
- Streamable HTTP transport (alongside stdio); push notifications via SSE
- `synapse-audio` MVP: WASAPI loopback + simple direction + Whisper-tiny STT

### Out of scope

Game profiles, debug overlay, VLM `describe`; the later hardware HID path was retired by #588/#589.

### Demo criterion

Agent registers `on_event` reflex: "when Save dialog appears, type path + Enter." Triggers via `act_press(["ctrl","s"])`; reflex fires; no intervention until saved.

### Risk areas

- Time-critical thread scheduling on Windows; debug jitter
- Profile hot-reload vs active state ordering
- Streamable HTTP / SSE re-connect semantics
- RocksDB Windows reliability; M3 uses RocksDB only per ADR-0002

---

## 6. M4 — First Game Profile + Compound Actions

**Goal:** add compound action/runtime surfaces and first-game profile work while
keeping input software-only.

### Scope

- `synapse-mcp` adds: `act_combo` (reflex scheduler), `act_run_shell` (gated), and `act_launch` (gated).
- First game profile work remains profile/HUD/keymap metadata only.
- Software input through `SendInput` and ViGEm is the supported strategy.
- The physical hardware-HID path, firmware, host crate, and CLI surfaces were retired by #588/#589.

### Out of scope

Multiple game profiles (M5), VLM `describe`, debug overlay, installer.

### Demo criterion

Agent + game/profile target. `observe()` returns HUD and visible entities.
Actions run through `software` or `vigem`; a requested `hardware` backend fails
closed with `ACTION_BACKEND_UNAVAILABLE`.

### Risk areas

- Detection accuracy on Minecraft (RT-DETRv2-S COCO is license-safe general detection; specialty fine-tune may be needed)
- HUD OCR for hearts/hunger via template-match; carefully cropped assets
- Software backend and ViGEm latency under sustained load; benchmark + tune

---

## 7. M5 — Production Polish (3-4 weeks)

**Goal:** v1.0 ship-ready.

M5 has one P1 strategic track that starts before the full M5 release gate:
the profile-registry / audit-data network effect from #454. Future agents must
not treat it as optional polish. The child issue set is #455-#470 and covers
local registry storage, package manifests, linked audit rows, MCP registry and
audit tools, signing/trust/rollback/quarantine, consent/redaction/export,
profile-quality scoring, authoring from audit/replay evidence, retention and
backfill, offline sync/contribution bundles, poisoning defenses, curated seed
profiles, inspector UI, shared-registry moderation, and governance/licensing.
The contribution-rights, attribution, provenance, and revocation baseline is
the operator-visible governance doc in
[`20_profile_registry_governance.md`](20_profile_registry_governance.md).
The optional shared-registry service/protocol and moderation boundary is
defined in
[`21_profile_registry_protocol.md`](21_profile_registry_protocol.md); local
registry use remains useful offline and does not require credentials.
The local-first storage/data-model baseline is
[`22_profile_registry_data_model.md`](22_profile_registry_data_model.md), which
pins `CF_PROFILES` row namespaces and `CF_KV` head pointers before runtime
registry tools land.
The profile package manifest baseline is
[`23_profile_package_manifest.md`](23_profile_package_manifest.md), which
defines package metadata, provenance, compatibility targets, permissions,
hashes, signed trust metadata, quarantine-only trust failures, rollback
verification, and fail-closed parser validation.
The curated starter registry baseline is
[`24_curated_starter_registry.md`](24_curated_starter_registry.md), which
defines seed set `starter.v1`, shipped/backlog targets #471-#482, and the
`curated_profile_target` row that package install writes when manifests carry
complete curated metadata.
The #460 privacy baseline adds explicit local consent rows in `CF_KV` plus
redacted local audit-export bundles with manifest, row hashes, and redaction
reports before any future contribution path can exist.

Physical sources of truth for this track are registry index/package files,
profile TOML files, RocksDB rows in `CF_ACTION_LOG`, `CF_REFLEX_AUDIT`,
`CF_EVENTS`, `CF_OBSERVATIONS`, `CF_SESSIONS`, and `CF_PROFILES`, consent and
export bundles, trust-root/quarantine/rollback rows, and MCP readbacks
(`profile_list`, `profile_quality_refresh`, `storage_inspect`, registry/audit
tools, `audit_export_consent_set`, and `audit_export_bundle`). Manual FSV must trigger
real Synapse runtime surfaces and then read those physical stores directly.
GitHub Actions/CI, scripts, tests, and benchmark harnesses are supporting
evidence only.

### Scope

- Installer (`SynapseSetup.msi`) via `wix-installer`
- Code signing (self-signed initially; project cert when funded)
- 5+ additional bundled profiles: `factorio`, `discord` / `slack`, `file_explorer`, `<one_fps>` (TBD, free game), `roblox_studio`
- Debug overlay (`synapse-overlay`)
- VLM-based `describe` (Florence-2-base ONNX)
- Full Grafana dashboards
- Local-first profile registry + audit-data learning loop (#454, #455-#470)
- Complete docs (this PRD + user-facing `USER_GUIDE.md`)
- Stable schema (v1 locked; future via migration / DB wipe)
- Performance budgets in `10_performance_budget.md` met on reference machine
- Soak test passes 8 hours clean
- Crash dump infrastructure
- `synapse-mcp setup` wizard
- Tray icon; license + token management
- Public release on GitHub Releases + crates.io; winget submission

### Demo criterion

Fresh Windows 11, no Synapse. Operator runs `synapse setup`, follows wizard, opens Claude Desktop. Agent:

1. Open VS Code, write small Rust file
2. `cargo build` via terminal
3. Switch to Chrome, search "Synapse MCP project," read result
4. Switch to Minecraft, play 2 minutes
5. Switch to music player, control playback

No screenshots; total token cost < 30K.

---

## 8. Post-v1

v1 ships at M5. v2+ priorities:

### v1.x patches

- Per-game fine-tuned detection models (`yolov10n_minecraft`, `yolov10n_factorio`)
- `Natural` aim curve improvements from feedback
- More bundled profiles via community

### v2 horizons

| Feature | Effort |
|---|---|
| Linux support (Wayland + AT-SPI) | ~6 weeks |
| macOS support (AX + ScreenCaptureKit + native input) | ~6 weeks |
| Cross-platform CDP (already half via `chromiumoxide`) | ~1 week |
| Per-game RAM hooks for sanctioned games (Minecraft mod API, KSP plugin) | ~2 weeks/game |
| Visual replay viewer (web app) | ~4 weeks |
| Profile marketplace (community-contributed with signing) | ~4 weeks |
| Steam Audio for spatial (HRTF replacement) | ~2 weeks |
| Sub-ms aim via PIO USB host on RP2040 (pass-through + corrections) | ~3 weeks |
| Browser DOM-only mode (structured-DOM RPA; no a11y/pixels) | ~2 weeks |

Not committed; v2 roadmap decided after v1 ships.

---

## 9. Risks and mitigations

| Milestone | Risk | Mitigation |
|---|---|---|
| M0 | rmcp API churn | Pin version; vet dep PRs |
| M1 | UIA performance | Cache request batching day one; fall back to depth-1 |
| M1 | DirectML on AMD/Intel | CPU fallback for detection; warn at startup if no GPU EP |
| M2 | ViGEm install friction | Document Win11 GUI clickthrough; auto-detect; if ViGEm is absent on the configured host, acquire/install it through local workflows before treating gamepad work as blocked |
| M3 | Time-critical thread jitter | Multimedia timer; fall back to `tokio::time` 2 ms tick if no MMCSS |
| M3 | RocksDB Windows hiccups | pinned RocksDB version; alternate backend requires future ADR |
| M4 | Retired hardware path leaves stale assumptions | Keep `hardware` token fail-closed and remove stale operator surfaces/docs |
| M4 | Minecraft detection accuracy | Mark accuracy honestly; commit to fine-tune in v1.x |
| M5 | MSI signing cert | Self-sign at v1.0; document SmartScreen warning; cert acquisition separate workstream |
| M5 | VLM bundle size | VLM optional download; `describe` returns `MODEL_NOT_LOADED` until downloaded |

---

## 10. Acceptance criteria

Release shippable when all true:

1. M0–M5 demos pass
2. Performance budgets in `10_performance_budget.md` met on reference machine (RTX 3060 + 8-core CPU)
3. Local configured-host checks and manual FSV green on the release candidate (no flakes in the exercised gates)
4. Soak test passes 8 hours
5. Manual test plan in `13_testing_strategy.md` §15 signed off
6. PRD internally consistent (no broken cross-refs)
7. License compliance clean (`cargo deny check`)
8. No `unsafe` outside documented allowed crates
9. No `unwrap()` outside tests (`#[deny(clippy::unwrap_used)]`)
10. Crash dumps verified on intentional panics

---

## 11. Out-of-bound items (not scheduled at v1)

- AI-driven profile authoring
- Cloud-hosted Synapse-as-a-service
- Multi-machine orchestration
- Mobile MCP clients driving Synapse remotely
- Sandbox / VM auto-provisioning
- Encrypted replay exports
- Real-time co-pilot (agent + human sharing input)

Fine v2+ ideas; cleanly outside v1.

---

## 12. The v1 promise

Operator gets:

- Signed Windows installer on PATH
- First-run wizard ≤ 5 minutes
- MCP server compatible with every major agent client
- ≤ 30 ms p99 `observe()` for productivity apps
- Real-time game support for ≥ 2 single-player titles
- Documented software-only input path for accessibility / research
- Complete PRD + user guide + reference docs
- Active GitHub Issues + Discussions community
- v1.x roadmap

The contract.

---

## 13. What this doc does NOT cover

- Issue tracker / project board (GitHub Projects)
- Sprint planning / iteration cadence
- Commercial roadmap
- Specific demo-game choices (finalized closer to M4)
