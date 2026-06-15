import assert from "node:assert/strict";
import { describe, test } from "node:test";

import { buildAgents, type DashboardState } from "../src/lib/dashboard-state";

function panel(data: unknown) {
  return { status: "ok" as const, source: "test", data };
}

function dashboardState(sessionsData: unknown): DashboardState {
  return {
    schema_version: 1,
    generated_at_unix_ms: 1,
    bind_addr: "127.0.0.1:7700",
    token_policy: "test",
    auth: panel({}),
    daemon: panel({}),
    sessions: panel(sessionsData),
    lease: panel({}),
    storage: panel({}),
    target_claims: panel({}),
    timeline: panel({}),
    events: panel({}),
    hidden_desktops: panel({}),
    cdp_attachments: panel({}),
    shell_jobs: panel({}),
    command_audit: panel({ rows: [] }),
    approvals: panel({}),
    suggestions: panel({}),
    armed_runs: panel({}),
    agent_transcripts: panel({ rows: [] }),
    hygiene: panel({}),
    local_models: panel({})
  };
}

function liveSession(state: string, reason = "session_initialized", lastSeenMs = 900_000) {
  return {
    session_id: `session-${state}-${reason}`,
    lifecycle: "live",
    agent_kind: "codex",
    last_seen_ms_ago: lastSeenMs,
    last_action: "tools/call:health",
    agent_state: {
      state,
      reason_code: reason
    }
  };
}

describe("buildAgents live session status", () => {
  test("keeps stale idle live sessions out of actionable attention", () => {
    const agents = buildAgents(
      dashboardState({
        sessions: [liveSession("idle", "session_initialized", 900_000)],
        unbound_agent_states: [],
        terminal_unbound_agent_states: []
      })
    );

    assert.equal(agents.length, 1);
    assert.equal(agents[0].status, "idle");
    assert.equal(agents[0].lastSeenMs, 900_000);
  });

  test("still surfaces explicit backend attention states", () => {
    const agents = buildAgents(
      dashboardState({
        sessions: [
          liveSession("stuck", "silent_timeout", 10),
          liveSession("needs_input", "operator_reply", 10),
          liveSession("awaiting_approval", "approval", 10),
          liveSession("ready_for_review", "review", 10)
        ],
        unbound_agent_states: []
      })
    );

    assert.deepEqual(agents.map((agent) => agent.status), [
      "stuck",
      "needs_input",
      "awaiting_approval",
      "ready_for_review"
    ]);
  });

  test("preserves terminal failure classification", () => {
    const agents = buildAgents(
      dashboardState({
        sessions: [liveSession("dead", "agent_spawn_failed", 900_000)],
        unbound_agent_states: []
      })
    );

    assert.equal(agents[0].status, "failed");
  });

  test("falls back to stale timing only for unknown live lifecycle rows", () => {
    const agents = buildAgents(
      dashboardState({
        sessions: [
          {
            session_id: "legacy-live-row",
            lifecycle: "live",
            agent_kind: "legacy",
            last_seen_ms_ago: 900_000,
            last_action: ""
          }
        ],
        unbound_agent_states: []
      })
    );

    assert.equal(agents[0].status, "stuck");
  });
});
