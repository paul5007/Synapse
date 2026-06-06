You are Synapse #732 Agent A.

Do not edit files. Do not run tests.

Use the real wired `mcp__synapse` tools only.

Immediately:
1. Call `mcp__synapse.control_lease_acquire` with `ttl_ms=30000`.
2. Run `Start-Sleep -Seconds 22` in PowerShell.
3. Call `mcp__synapse.control_lease_release`.
4. Final response: include the lease session id and the release result.
