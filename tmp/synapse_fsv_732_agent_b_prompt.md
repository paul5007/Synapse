You are Synapse #732 manual verification Agent B.

Do not edit files. Do not run tests. Do not use workarounds.

Use the real wired `mcp__synapse` MCP tools. If the tools are not visible, discover them with tool search for `synapse act_click control_lease observe find health`.

Known synthetic targets:
- CDP Chrome window HWND: 396980
- CDP button element id: 0x60eb4:cdcd2a003485cf0ab28ac019d629de302fc200000000000a
- CDP expected before text: count=0
- CDP expected after text: count=1
- UIA WinForms window HWND: 1576774
- UIA button element id: 0x990c5a:0000002a00990c5a
- UIA file Source of Truth: C:\code\Synapse\tmp\synapse_fsv_732_uia_result.txt
- UIA expected before file content: invoked=0
- UIA expected after file content starts with: invoked=1

Task:
1. Call `mcp__synapse.health`.
2. Call `mcp__synapse.control_lease_status` and print `B_BEFORE_STATUS`. Expected: held by another session, not this session.
3. Read the UIA file with PowerShell `Get-Content -LiteralPath 'C:\code\Synapse\tmp\synapse_fsv_732_uia_result.txt'` and print `B_UIA_FILE_BEFORE`.
4. Call `mcp__synapse.find` for `count=0` in `window_hwnd=396980`, scope `elements`, limit `5`; print `B_CDP_COUNT_BEFORE`.
5. Call `mcp__synapse.act_click` on the CDP button element id above with `button=left`, `clicks=1`, `verify_delta=false`. Print the full response as `B_CDP_CLICK_RESPONSE`, especially `backend_tier_used` and `required_foreground`.
6. Call `mcp__synapse.find` for `count=1` in `window_hwnd=396980`, scope `elements`, limit `5`; print `B_CDP_COUNT_AFTER`.
7. Call `mcp__synapse.control_lease_status`; print `B_AFTER_CDP_STATUS`.
8. Call `mcp__synapse.act_click` on the UIA button element id above with `button=left`, `clicks=1`, `use_invoke_pattern=true`, `verify_delta=false`. Print the full response as `B_UIA_CLICK_RESPONSE`, especially `backend_tier_used` and `required_foreground`.
9. Read the UIA file again and print `B_UIA_FILE_AFTER`.
10. Call `mcp__synapse.control_lease_status`; print `B_AFTER_UIA_STATUS`.
11. Call `mcp__synapse.act_click` on coordinate `{ "x": 12, "y": 34 }` with `button=left`, `clicks=1`, `verify_delta=false`. This is expected to fail while Agent A holds the lease. Print the full error as `B_COORDINATE_BUSY_ERROR`, especially code `ACTION_FOREGROUND_LEASE_BUSY`, holder session id, requesting session id, and retry hint.
12. Call `mcp__synapse.control_lease_status`; print `B_AFTER_COORDINATE_STATUS`.

Keep the final response concise and include whether the CDP and UIA actions succeeded while the lease holder remained Agent A.
