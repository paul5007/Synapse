You are Synapse #732 Agent A.

Do not edit files. Do not run tests.

Use the Synapse MCP tool `control_lease_acquire` with `ttl_ms=30000` now.
Then use the shell to run:

pwsh -NoLogo -NoProfile -NonInteractive -Command "Start-Sleep -Seconds 25"

Then use the Synapse MCP tool `control_lease_release`.

Return the acquire result, release result, and the Synapse session id.
