#!/usr/bin/env python3
"""Run an internalized local-model agent swarm against a live Synapse daemon.

This is an operational probe, not an acceptance substitute. It performs a real
MCP initialize/tools-list handshake, spawns sampled local-model agents through
act_spawn_agent, and verifies each sampled agent by reading back its exact
workspace row with workspace_get.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import json
import os
import sys
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any


DEFAULT_MCP_URL = "http://127.0.0.1:7700/mcp"
DEFAULT_MODEL_REF = "qwen8v2-tool-live"
REQUIRED_TOOLS = {
    "act_spawn_agent",
    "approval_decide",
    "approval_list",
    "local_model_list",
    "workspace_get",
}


class SwarmError(RuntimeError):
    """A fail-closed error with enough context for operator diagnosis."""


def _appdata_token_path() -> Path:
    appdata = os.environ.get("APPDATA")
    if not appdata:
        raise SwarmError("APPDATA is not set and SYNAPSE_BEARER_TOKEN is absent")
    return Path(appdata) / "synapse" / "token.txt"


def resolve_bearer_token() -> str:
    token = os.environ.get("SYNAPSE_BEARER_TOKEN", "").strip()
    if token:
        return token
    token_path = _appdata_token_path()
    try:
        token = token_path.read_text(encoding="ascii").strip()
    except FileNotFoundError as error:
        raise SwarmError(
            f"SYNAPSE bearer token missing: env SYNAPSE_BEARER_TOKEN is empty "
            f"and {token_path} does not exist"
        ) from error
    if not token:
        raise SwarmError(f"SYNAPSE bearer token file is empty: {token_path}")
    return token


def parse_sse_or_json(raw: str) -> dict[str, Any]:
    last_data = None
    for line in raw.splitlines():
        stripped = line.strip()
        if stripped.startswith("data:"):
            last_data = stripped[len("data:") :].strip()
    if last_data:
        return json.loads(last_data)
    stripped = raw.strip()
    if not stripped:
        raise SwarmError("empty MCP response body")
    return json.loads(stripped)


def extract_tool_value(response: dict[str, Any]) -> Any:
    if "error" in response:
        raise SwarmError(f"MCP tool error: {json.dumps(response['error'], sort_keys=True)}")
    result = response.get("result")
    if not isinstance(result, dict):
        raise SwarmError(f"MCP response missing result object: {response!r}")
    if "structuredContent" in result:
        return result["structuredContent"]
    content = result.get("content")
    if isinstance(content, list):
        for item in content:
            if isinstance(item, dict) and item.get("type") == "text":
                text = item.get("text")
                if isinstance(text, str):
                    try:
                        return json.loads(text)
                    except json.JSONDecodeError:
                        return text
    return result


class McpHttpClient:
    def __init__(self, url: str, token: str, client_name: str) -> None:
        self.url = url
        self.token = token
        self.client_name = client_name
        self.session_id: str | None = None
        self._next_id = 1

    def request(
        self,
        method: str,
        params: dict[str, Any] | None = None,
        *,
        notification: bool = False,
        timeout_s: float = 60.0,
    ) -> dict[str, Any] | None:
        body: dict[str, Any] = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            body["params"] = params
        if not notification:
            body["id"] = self._next_id
            self._next_id += 1
        data = json.dumps(body, separators=(",", ":")).encode("utf-8")
        headers = {
            "Authorization": f"Bearer {self.token}",
            "Content-Type": "application/json",
            "Accept": "application/json, text/event-stream",
        }
        if self.session_id:
            headers["Mcp-Session-Id"] = self.session_id
        req = urllib.request.Request(self.url, data=data, headers=headers, method="POST")
        try:
            with urllib.request.urlopen(req, timeout=timeout_s) as resp:
                session_id = resp.headers.get("Mcp-Session-Id")
                if session_id:
                    self.session_id = session_id
                raw = resp.read().decode("utf-8", "replace")
        except urllib.error.HTTPError as error:
            raw_error = error.read().decode("utf-8", "replace")
            raise SwarmError(
                f"MCP HTTP {error.code} for {method}: {raw_error[:1000]}"
            ) from error
        except urllib.error.URLError as error:
            raise SwarmError(f"MCP transport failed for {method}: {error}") from error
        if notification:
            return None
        return parse_sse_or_json(raw)

    def initialize(self) -> list[dict[str, Any]]:
        init = self.request(
            "initialize",
            {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": self.client_name, "version": "1.0"},
            },
            timeout_s=30,
        )
        if not self.session_id:
            raise SwarmError("MCP initialize did not return Mcp-Session-Id")
        if not init or "result" not in init:
            raise SwarmError(f"MCP initialize failed: {init!r}")
        self.request("notifications/initialized", {}, notification=True, timeout_s=10)
        listed = self.request("tools/list", {}, timeout_s=30)
        if not listed or "result" not in listed:
            raise SwarmError(f"MCP tools/list failed: {listed!r}")
        tools = listed["result"].get("tools")
        if not isinstance(tools, list):
            raise SwarmError(f"MCP tools/list returned non-list tools: {listed!r}")
        return tools

    def call_tool(
        self, name: str, arguments: dict[str, Any], *, timeout_s: float = 60.0
    ) -> Any:
        response = self.request(
            "tools/call",
            {"name": name, "arguments": arguments},
            timeout_s=timeout_s,
        )
        if response is None:
            raise SwarmError(f"MCP tools/call {name} returned no response")
        return extract_tool_value(response)


@dataclass(frozen=True)
class AgentSpec:
    index: int
    run_id: str
    key: str
    value: dict[str, Any]
    prompt: str


@dataclass(frozen=True)
class AgentResult:
    index: int
    key: str
    ok: bool
    elapsed_s: float
    spawn_id: str | None
    session_id: str | None
    matched: bool
    approvals_accepted: tuple[str, ...]
    error: str | None
    value_sha256: str | None


def build_spec(run_id: str, index: int) -> AgentSpec:
    key = f"sample/agent-{index:04d}"
    value = {
        "issue": 1056,
        "agent_index": index,
        "expected": (index + 17) * 3,
        "verdict": "swarm-executed",
    }
    tool_arguments = {"run_id": run_id, "key": key, "value": value}
    prompt = (
        "Return no prose until after the tool succeeds. "
        "Use exactly one Synapse MCP tool call named workspace_put. "
        "The workspace_put arguments object must be exactly: "
        f"{json.dumps(tool_arguments, sort_keys=True, separators=(',', ':'))}. "
        "Do not include expected_version. Do not invent tools. "
        "After the tool result includes post_write_readback.matched=true, "
        f"reply with verified {key}."
    )
    return AgentSpec(index=index, run_id=run_id, key=key, value=value, prompt=prompt)


def workspace_put_arguments(spec: AgentSpec) -> dict[str, Any]:
    return {"run_id": spec.run_id, "key": spec.key, "value": spec.value}


def oracle_validate(specs: list[AgentSpec]) -> tuple[int, list[str]]:
    errors: list[str] = []
    keys: set[tuple[str, str]] = set()
    for spec in specs:
        ident = (spec.run_id, spec.key)
        if ident in keys:
            errors.append(f"duplicate workspace key: run_id={spec.run_id} key={spec.key}")
        keys.add(ident)
        try:
            json.dumps(spec.value, sort_keys=True)
        except (TypeError, ValueError) as error:
            errors.append(f"agent {spec.index} value is not JSON encodable: {error}")
        if "workspace_put" not in spec.prompt:
            errors.append(f"agent {spec.index} prompt does not name workspace_put")
        if spec.key not in spec.prompt:
            errors.append(f"agent {spec.index} prompt does not include exact key")
        if spec.run_id not in spec.prompt:
            errors.append(f"agent {spec.index} prompt does not include exact run_id")
    return len(specs) - len(errors), errors


def select_sample_indices(agent_count: int, sample_count: int) -> list[int]:
    if agent_count < 1:
        raise SwarmError("--agents must be >= 1")
    if sample_count < 1:
        raise SwarmError("--execute-sample must be >= 1")
    if sample_count > agent_count:
        raise SwarmError("--execute-sample cannot exceed --agents")
    if sample_count == agent_count:
        return list(range(agent_count))
    if sample_count == 1:
        return [0]
    max_index = agent_count - 1
    return sorted({round(i * max_index / (sample_count - 1)) for i in range(sample_count)})


def validate_required_tools(tools: list[dict[str, Any]]) -> dict[str, Any]:
    by_name = {tool.get("name"): tool for tool in tools if isinstance(tool, dict)}
    missing = sorted(name for name in REQUIRED_TOOLS if name not in by_name)
    if missing:
        raise SwarmError(f"MCP tools/list missing required tools: {missing}")
    for name in sorted(REQUIRED_TOOLS):
        schema = by_name[name].get("inputSchema")
        if schema is True or schema is None:
            raise SwarmError(
                f"MCP tools/list returned an invalid inputSchema for {name}: {schema!r}"
            )
    return by_name


def validate_model_row(client: McpHttpClient, model_ref: str) -> dict[str, Any]:
    result = client.call_tool(
        "local_model_list",
        {"name": model_ref, "include_disabled": True, "limit": 10},
        timeout_s=30,
    )
    rows = result.get("rows") if isinstance(result, dict) else None
    if not isinstance(rows, list):
        raise SwarmError(f"local_model_list returned invalid rows: {result!r}")
    matches = [row for row in rows if isinstance(row, dict) and row.get("name") == model_ref]
    if len(matches) != 1:
        raise SwarmError(f"local_model_list found {len(matches)} rows for {model_ref!r}")
    row = matches[0]
    if row.get("enabled") is not True:
        raise SwarmError(f"local model {model_ref!r} is disabled")
    probe = row.get("last_probe")
    if not isinstance(probe, dict) or probe.get("healthy") is not True:
        raise SwarmError(f"local model {model_ref!r} is not healthy: {probe!r}")
    if row.get("runtime_preset") != "internalized_no_catalog":
        raise SwarmError(
            f"local model {model_ref!r} is not internalized_no_catalog: "
            f"{row.get('runtime_preset')!r}"
        )
    return row


def find_spawn_identity(spawn_result: Any) -> tuple[str | None, str | None]:
    if not isinstance(spawn_result, dict):
        return None, None
    spawn_id = (
        spawn_result.get("spawn_id")
        or spawn_result.get("id")
        or spawn_result.get("agent_spawn_id")
    )
    session_id = (
        spawn_result.get("session_id")
        or spawn_result.get("mcp_session_id")
        or spawn_result.get("agent_session_id")
    )
    if not session_id:
        readback = spawn_result.get("readback")
        if isinstance(readback, dict):
            session_id = readback.get("session_id") or readback.get("mcp_session_id")
    return (
        spawn_id if isinstance(spawn_id, str) else None,
        session_id if isinstance(session_id, str) else None,
    )


def read_workspace_value(client: McpHttpClient, run_id: str, key: str) -> tuple[Any, str | None]:
    result = client.call_tool(
        "workspace_get", {"run_id": run_id, "key": key}, timeout_s=30
    )
    if not isinstance(result, dict):
        raise SwarmError(f"workspace_get returned non-object for {key}: {result!r}")
    entry = result.get("entry")
    if not isinstance(entry, dict):
        raise SwarmError(f"workspace_get missing entry for {key}: {result!r}")
    storage_readback = result.get("storage_readback")
    value_sha256 = None
    if isinstance(storage_readback, dict):
        value_sha256 = storage_readback.get("value_sha256")
    return entry.get("value"), value_sha256 if isinstance(value_sha256, str) else None


def approval_payload(item_wrapper: Any) -> tuple[str, dict[str, Any]] | None:
    if not isinstance(item_wrapper, dict):
        return None
    item = item_wrapper.get("item")
    if not isinstance(item, dict):
        return None
    approval_id = item.get("approval_id")
    payload_raw = item.get("payload_json")
    if not isinstance(approval_id, str) or not isinstance(payload_raw, str):
        return None
    try:
        payload = json.loads(payload_raw)
    except json.JSONDecodeError:
        return None
    if not isinstance(payload, dict):
        return None
    return approval_id, payload


def maybe_accept_exact_workspace_put(
    client: McpHttpClient, spec: AgentSpec, spawn_id: str | None
) -> str | None:
    pending = client.call_tool(
        "approval_list",
        {"statuses": ["pending"], "limit": 100},
        timeout_s=30,
    )
    items = pending.get("items") if isinstance(pending, dict) else None
    if not isinstance(items, list):
        raise SwarmError(f"approval_list returned invalid items: {pending!r}")
    expected_input = workspace_put_arguments(spec)
    for item_wrapper in items:
        decoded = approval_payload(item_wrapper)
        if decoded is None:
            continue
        approval_id, payload = decoded
        if payload.get("tool_name") != "mcp__synapse__workspace_put":
            continue
        payload_spawn_id = payload.get("spawn_id")
        same_spawn = spawn_id is not None and payload_spawn_id == spawn_id
        exact_input = payload.get("input") == expected_input
        if not same_spawn and not exact_input:
            continue
        if exact_input:
            client.call_tool(
                "approval_decide",
                {
                    "approval_id": approval_id,
                    "decision": "accept",
                    "note": (
                        "swarm.py exact workspace_put auto-approval: "
                        f"run_id={spec.run_id} key={spec.key}"
                    ),
                },
                timeout_s=30,
            )
            return approval_id
        client.call_tool(
            "approval_decide",
            {
                "approval_id": approval_id,
                "decision": "decline",
                "note": (
                    "swarm.py declined workspace_put because args did not "
                    f"match run_id={spec.run_id} key={spec.key}"
                ),
            },
            timeout_s=30,
        )
        raise SwarmError(
            "workspace_put approval args mismatch: "
            f"expected={json.dumps(expected_input, sort_keys=True)} "
            f"actual={json.dumps(payload.get('input'), sort_keys=True)}"
        )
    return None


def run_agent(
    spec: AgentSpec,
    *,
    mcp_url: str,
    token: str,
    model_ref: str,
    working_dir: str,
    hold_open_ms: int,
    spawn_wait_ms: int,
    readback_timeout_ms: int,
    poll_ms: int,
) -> AgentResult:
    started = time.perf_counter()
    client = McpHttpClient(
        mcp_url,
        token,
        f"synapse-swarm-agent-{spec.index:04d}",
    )
    try:
        tools = client.initialize()
        validate_required_tools(tools)
        spawn_result = client.call_tool(
            "act_spawn_agent",
            {
                "cli": "local_model",
                "model_ref": model_ref,
                "prompt": spec.prompt,
                "working_dir": working_dir,
                "mcp_url": mcp_url,
                "wait_timeout_ms": spawn_wait_ms,
                "hold_open_ms": hold_open_ms,
                "require_approval_gate": True,
            },
            timeout_s=max(30.0, spawn_wait_ms / 1000.0 + 30.0),
        )
        spawn_id, session_id = find_spawn_identity(spawn_result)
        deadline = time.perf_counter() + (readback_timeout_ms / 1000.0)
        last_error: str | None = None
        approvals_accepted: list[str] = []
        while time.perf_counter() < deadline:
            try:
                accepted = maybe_accept_exact_workspace_put(client, spec, spawn_id)
                if accepted and accepted not in approvals_accepted:
                    approvals_accepted.append(accepted)
                value, value_sha256 = read_workspace_value(client, spec.run_id, spec.key)
                matched = value == spec.value
                return AgentResult(
                    index=spec.index,
                    key=spec.key,
                    ok=matched,
                    elapsed_s=time.perf_counter() - started,
                    spawn_id=spawn_id,
                    session_id=session_id,
                    matched=matched,
                    approvals_accepted=tuple(approvals_accepted),
                    error=None if matched else f"value mismatch: {value!r}",
                    value_sha256=value_sha256 if isinstance(value_sha256, str) else None,
                )
            except Exception as error:  # noqa: BLE001 - preserve exact transient read failure.
                last_error = str(error)
                time.sleep(poll_ms / 1000.0)
        return AgentResult(
            index=spec.index,
            key=spec.key,
            ok=False,
            elapsed_s=time.perf_counter() - started,
            spawn_id=spawn_id,
            session_id=session_id,
            matched=False,
            approvals_accepted=tuple(approvals_accepted),
            error=f"readback timeout: {last_error}",
            value_sha256=None,
        )
    except Exception as error:  # noqa: BLE001 - top-level worker reports all failures.
        return AgentResult(
            index=spec.index,
            key=spec.key,
            ok=False,
            elapsed_s=time.perf_counter() - started,
            spawn_id=None,
            session_id=None,
            matched=False,
            approvals_accepted=(),
            error=str(error),
            value_sha256=None,
        )


def positive_int(raw: str) -> int:
    value = int(raw)
    if value < 1:
        raise argparse.ArgumentTypeError("must be >= 1")
    return value


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Spawn sampled internalized local-model agents and verify their "
            "workspace_put effects through workspace_get."
        )
    )
    parser.add_argument("--mcp-url", default=DEFAULT_MCP_URL)
    parser.add_argument("--model-ref", default=DEFAULT_MODEL_REF)
    parser.add_argument("--agents", type=positive_int, default=3)
    parser.add_argument("--execute-sample", type=positive_int, default=3)
    parser.add_argument("--concurrency", type=positive_int, default=1)
    parser.add_argument("--run-id")
    parser.add_argument("--working-dir", default=str(Path.cwd()))
    parser.add_argument("--hold-open-ms", type=positive_int, default=5_000)
    parser.add_argument("--spawn-wait-ms", type=positive_int, default=180_000)
    parser.add_argument("--readback-timeout-ms", type=positive_int, default=120_000)
    parser.add_argument("--poll-ms", type=positive_int, default=1_000)
    parser.add_argument("--report-path")
    return parser


def main(argv: list[str]) -> int:
    args = build_parser().parse_args(argv)
    run_id = args.run_id or f"swarm-{time.strftime('%Y%m%dT%H%M%S')}"
    token = resolve_bearer_token()

    setup_client = McpHttpClient(args.mcp_url, token, "synapse-swarm-setup")
    tools = setup_client.initialize()
    validate_required_tools(tools)
    model_row = validate_model_row(setup_client, args.model_ref)

    specs = [build_spec(run_id, index) for index in range(args.agents)]
    oracle_valid, oracle_errors = oracle_validate(specs)
    if oracle_errors:
        raise SwarmError("oracle validation failed: " + "; ".join(oracle_errors))

    sample_indices = select_sample_indices(args.agents, args.execute_sample)
    sampled_specs = [specs[index] for index in sample_indices]
    started = time.perf_counter()
    max_workers = min(args.concurrency, len(sampled_specs))
    results: list[AgentResult] = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=max_workers) as pool:
        futures = [
            pool.submit(
                run_agent,
                spec,
                mcp_url=args.mcp_url,
                token=token,
                model_ref=args.model_ref,
                working_dir=args.working_dir,
                hold_open_ms=args.hold_open_ms,
                spawn_wait_ms=args.spawn_wait_ms,
                readback_timeout_ms=args.readback_timeout_ms,
                poll_ms=args.poll_ms,
            )
            for spec in sampled_specs
        ]
        for future in concurrent.futures.as_completed(futures):
            results.append(future.result())
    elapsed_s = time.perf_counter() - started
    results.sort(key=lambda result: result.index)
    executed_verified = sum(1 for result in results if result.ok and result.matched)
    agents_s = executed_verified / elapsed_s if elapsed_s > 0 else 0.0

    report = {
        "run_id": run_id,
        "model_ref": args.model_ref,
        "model_runtime_preset": model_row.get("runtime_preset"),
        "agents": args.agents,
        "execute_sample": len(sampled_specs),
        "sample_indices": sample_indices,
        "concurrency": max_workers,
        "oracle_valid": oracle_valid,
        "executed_verified": executed_verified,
        "agents_s": agents_s,
        "elapsed_s": elapsed_s,
        "mcp_url": args.mcp_url,
        "mcp_session_id": setup_client.session_id,
        "required_tools": sorted(REQUIRED_TOOLS),
        "results": [
            {
                "index": result.index,
                "key": result.key,
                "ok": result.ok,
                "matched": result.matched,
                "elapsed_s": result.elapsed_s,
                "spawn_id": result.spawn_id,
                "session_id": result.session_id,
                "approvals_accepted": list(result.approvals_accepted),
                "value_sha256": result.value_sha256,
                "error": result.error,
            }
            for result in results
        ],
    }

    encoded = json.dumps(report, indent=2, sort_keys=True)
    print(encoded)
    if args.report_path:
        report_path = Path(args.report_path)
        report_path.parent.mkdir(parents=True, exist_ok=True)
        report_path.write_text(encoded + "\n", encoding="utf-8")

    if executed_verified != len(sampled_specs):
        return 1
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main(sys.argv[1:]))
    except SwarmError as error:
        print(f"SWARM_ERROR: {error}", file=sys.stderr)
        raise SystemExit(2)
