# RECOVERY NOTES - Synapse

## Current Resume Point - 2026-06-01T05:44:31-05:00
- #612 is closed with commit `db761fe` and RESOLVED evidence at https://github.com/ChrisRoyse/Synapse/issues/612#issuecomment-4591828569. Closure readback: state `CLOSED`, closed at `2026-06-01T10:43:56Z`.
- Live open queue after closing #612: #594 plus #595-#604 and #613-#634.
- Active issue: #613 `scenario(stress): subscribe firehose - 4096 ring, EVENTS_DROPPED, one-per-event, deep filters`.
- START comment: https://github.com/ChrisRoyse/Synapse/issues/613#issuecomment-4591831842
- #613 acceptance requires real MCP `tools/call` triggers plus separate physical SoT readbacks for:
  - `subscribe` with `snapshot_first` across many event kinds;
  - storm generation through rapid UIA changes, process churn via shell, filesystem writes in a watch root, and clipboard changes;
  - one `synapse/event` notification per event;
  - 4096 ring bound and `EVENTS_DROPPED` metric/drop accounting under slow consumer/backpressure;
  - 8-level-deep filter with regex/in_set/exists predicates;
  - filter depth 9 rejection;
  - subscribe then immediate cancel;
  - empty filter/All, boundary, and structurally invalid params.
- Suggested SoTs: repo-built daemon process/socket/auth/session/tool-list, SSE stream bytes or MCP notification delivery, delivered event count and ids, `storage_inspect` / `CF_EVENTS` rows, event bus drop metrics, log bytes, physical clipboard/file/process/UI changes used as event causes, and cleanup state.

## Next Steps
1. Re-read this file and #613 after any compaction.
2. Inspect subscribe/SSE/event-bus implementation and existing tests before editing.
3. Patch only if code inspection or manual runtime evidence exposes gaps.
4. Build repo runtime, launch isolated #613 daemon, prove process/socket/auth/health and strict Inspector tools-list.
5. Run #613 manual FSV with real MCP/SSE triggers and separate physical SoT readbacks.

## Standing Rules
- No GitHub Actions/CI dispatch, waits, or CI-gated claims.
- Commits pushed by this agent must include `[skip ci]`.
- Automated checks/benches are supporting regression evidence only; they are not FSV.
- Missing local prerequisites are acquisition/setup work, not blockers, unless only a specific operator-only hard-to-reverse external action remains.
