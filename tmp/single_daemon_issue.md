## Problem

The multi-agent design (epic #717) depends on a **single canonical daemon**: the process-global
input lease (#719), per-session target registry (#720), and per-session held-input ledger only
coordinate agents if every MCP client talks to the *same* daemon process. If a second daemon can
start, each gets its own in-process lease/registries and isolation silently breaks. The requirement
is absolute: **reconnect, PC restart, and opening multiple terminals must never produce more than one
daemon.**

## Root cause (verified in source)

`--mode http` already acquires a single-instance guard before binding the port or opening RocksDB
(`http/transport.rs` → `single_instance::SingleInstanceGuard::acquire`, OS advisory file lock on
`<db>/daemon.lock`, released automatically on process death; PID recorded in `<db>/daemon.pid`). The
fixed loopback port `127.0.0.1:7700` is itself an exclusive singleton for HTTP, and the connect
bridge (`--mode connect`) probes `/health` and spawns the daemon at most once (lock breaks the race).

**The gap:** `--mode stdio` (the default mode) is *also a full daemon* — it opens RocksDB and owns
its own process-global input lease + session registries — but it did **not** acquire the
single-instance guard. A stray or misconfigured stdio launch (e.g. an agent CLI whose MCP config uses
stdio instead of HTTP/connect) would either crash later on a cryptic RocksDB `LOCK` error (same DB) or
run a fully independent parallel daemon (different DB) — breaking the single-daemon invariant.

## Fix

`run_stdio` now acquires the same `SingleInstanceGuard` (keyed on the resolved DB path) before
building the service, and fails loud — exit code 3, `code=MCP_DAEMON_ALREADY_RUNNING` naming the
holder PID, with an actionable message pointing at `--mode connect` — instead of starting a second
daemon. The guard is held for the process lifetime and released on exit (and by the OS on crash).
The guard stays **per-DB-path** so legitimate separate-DB daemons (tests/secondary instances) still
work; the canonical daemon is additionally protected by the exclusive `127.0.0.1:7700` port bind.

Files: `crates/synapse-mcp/src/main.rs` (`run_stdio`), regression tests added to
`crates/synapse-mcp/src/single_instance.rs`.

## Full-State-Verification (real processes, source of truth = `daemon.pid` + process list)

Canonical daemon running: pid **39140**, `--mode http --bind 127.0.0.1:7700 --db ...\db-daemon`;
`daemon.pid` = 39140 (source of truth).

1. **Happy path — stdio refuses on canonical DB.** Launched the freshly-built binary
   `--mode stdio --db <canonical>`:
   - exit code = **3** (expected)
   - stderr: `code="MCP_DAEMON_ALREADY_RUNNING" holder_pid=39140 mode="stdio"` + the actionable message
   - after: process list still shows **only 39140** — no parallel daemon was created. ✅
2. **Edge — http refuses on canonical DB (different port 7799).** exit code = **3**;
   `MCP_DAEMON_ALREADY_RUNNING holder_pid="39140"` — refused by the lock *before* port bind. ✅
3. **Edge — stdio on a DIFFERENT temp DB is allowed.** `--mode stdio --db <temp>` →
   `code="MCP_DAEMON_SINGLE_INSTANCE_ACQUIRED" mode="stdio"` (pid 65592) and ran — proving the guard
   is correctly per-DB-scoped, not over-broad. Killed cleanly; final process list = only 39140. ✅

Unit regression tests (real filesystem, no mocks), both green:
- `second_acquire_same_db_is_refused_then_frees_on_drop` — second acquire → `AlreadyRunning` naming
  this PID (read back from the `daemon.pid` sidecar); lock frees after the holder drops.
- `different_db_paths_acquire_independently`.

`cargo clippy -p synapse-mcp` clean; `cargo test -p synapse-mcp --bins single_instance` = 3 passed.

## Result

It is now impossible for a second daemon to start against the canonical DB/port via any mode
(stdio or http); the connect bridge already converges stdio-only clients onto the one daemon. A
reconnect re-initializes a session on the *same* daemon (an orphaned per-session lease self-heals via
its TTL, the #719 safety net); a PC restart starts exactly one daemon (first http/connect wins the
lock + port); multiple terminals all converge on `127.0.0.1:7700`.
