#!/usr/bin/env python3
"""Authored wire fixture. It never imports or invokes a provider SDK."""
import base64
import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import time

fixture_root = Path(os.environ["CLAUDE_CONFIG_DIR"])
fixture = json.loads((fixture_root / "fixture.json").read_text())
mode = fixture["mode"]
record = fixture_root / "requests.jsonl"
assert os.environ.get("HOME") == fixture.get("home")
args = sys.argv[1:]
with record.open("a") as out:
    out.write(json.dumps({"args": args, "cwd": os.getcwd(), "pid": os.getpid(), "home": os.environ.get("HOME"), "mode": stat.S_IMODE(Path.cwd().stat().st_mode)}) + "\n")
assert stat.S_IMODE(Path.cwd().stat().st_mode) == 0o700
assert not any(k in os.environ for k in ["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN", "ANTHROPIC_BASE_URL"])
if args == ["--version"]:
    print("2.1.283 (Claude Code)" if mode == "wrong-version" else "2.1.284 (Claude Code)")
    sys.exit(0)
if args == ["auth", "status"]:
    print(json.dumps({"loggedIn": mode != "signed-out", "authMethod": "apiKey" if mode == "byok-status" else "claude.ai", "apiProvider": "firstParty", "email": "fixture@example.invalid", "subscriptionType": "pro", "unexpected_token": "do-not-echo"}, indent=2))
    sys.exit(1 if mode == "signed-out" else 0)
required = ["--print", "--safe-mode", "--restricted", "--tools", "--disallowedTools", "--permission-prompts", "--strict-mcp-config", "--setting-sources=", "--no-session-persistence", "--no-chrome", "--disable-slash-commands"]
assert all(a in args for a in required)
assert args[args.index("--tools") + 1] == ""
assert args[args.index("--disallowedTools") + 1] == "*"
assert args[args.index("--permission-prompts") + 1] == "none"
assert args[args.index("--max-turns") + 1] == "1"
assert "--bare" not in args
assert "--dangerously-skip-permissions" not in args
for flag in ["--system-prompt-file", "--settings", "--mcp-config"]:
    p = Path(args[args.index(flag) + 1]); assert p.parent.resolve() == Path.cwd()
    assert stat.S_IMODE(p.stat().st_mode) == 0o600
system = Path(args[args.index("--system-prompt-file") + 1]).read_text()
assert json.loads(Path(args[args.index("--mcp-config") + 1]).read_text()) == {"mcpServers": {}}

def emit(v):
    print(json.dumps(v, ensure_ascii=False), flush=True)

first = json.loads(sys.stdin.readline())
assert first == {"type": "control_request", "request_id": "markitai-initialize", "request": {"subtype": "initialize", "hooks": None, "agents": {}, "skills": []}}
if mode == "callback-init":
    emit({"type": "control_request", "request_id": "unexpected", "request": {"subtype": "can_use_tool"}})
    time.sleep(30)
if mode == "hang-init":
    time.sleep(30)
if mode == "stderr-flood":
    sys.stderr.write("secret" * 200000); sys.stderr.flush(); time.sleep(30)
account = {"apiProvider": "bedrock" if mode == "wrong-provider" else "firstParty", "subscriptionType": "pro", "apiKeySource": "none", "email": "fixture@example.invalid"}
emit({"type": "control_response", "response": {"subtype": "success", "request_id": "markitai-initialize", "response": {"models": [{"value": "sonnet", "resolvedModel": "claude-fixture-1", "displayName": "Fixture Claude", "description": "local only"}], "account": account}}})
line = sys.stdin.readline()
if not line:
    sys.exit(0)
user = json.loads(line)
assert user["type"] == "user" and user["message"]["role"] == "user"
assert user["parent_tool_use_id"] is None and user["session_id"] == ""
content = user["message"]["content"]
assert content[0]["type"] == "text"
images = []
for c in content[1:]:
    assert c["type"] == "image" and c["source"]["type"] == "base64"
    b = base64.b64decode(c["source"]["data"], validate=True)
    images.append({"mime": c["source"]["media_type"], "sha256": hashlib.sha256(b).hexdigest()})
with record.open("a") as out:
    out.write(json.dumps({"request": {"system": system, "text": content[0]["text"], "images": images}}) + "\n")
assert sys.stdin.read() == ""
emit({"type": "system", "subtype": "init", "session_id": "fixture-session", "claude_code_version": "2.1.284", "apiKeySource": "none", "tools": [], "mcp_servers": [], "agents": [], "skills": [], "plugins": [], "permissionMode": "dontAsk"})
emit({"type": "system", "subtype": "session_state_changed", "state": "running", "session_id": "fixture-session"})
usage = {"input_tokens": 11, "output_tokens": 7, "cache_read_input_tokens": 3, "cache_creation_input_tokens": 2}
assistant = {"type": "assistant", "session_id": "fixture-session", "parent_tool_use_id": None, "message": {"id": "msg-one", "model": "claude-fixture-1", "content": [{"type": "text", "text": "partial is not final"}], "usage": usage, "stop_reason": "end_turn"}}
if mode == "tool":
    assistant["message"]["content"] = [{"type": "tool_use", "id": "tool-one", "name": "Bash", "input": {"command": "forbidden"}}]
if mode == "wrong-model":
    assistant["message"]["model"] = "unrequested-model"
if mode not in ["aggregate-only", "aggregate-paid-error", "unknown-usage"]:
    emit(assistant)
if mode == "duplicate":
    emit(assistant)
if mode == "conflict":
    assistant["message"]["usage"]["output_tokens"] = 2; emit(assistant)
if mode == "paid-callback":
    emit({"type": "control_request", "request_id": "bad", "request": {"subtype": "can_use_tool"}}); time.sleep(30)
if mode == "missing-terminal":
    sys.exit(0)
if mode == "hang-paid":
    subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"], stdin=subprocess.DEVNULL)
    time.sleep(30)
if mode == "output-limit":
    print("x" * (16 * 1024 * 1024 + 32), flush=True); time.sleep(30)
if mode == "malformed-paid":
    print('{"bad":', flush=True); sys.exit(0)
result = {"type": "result", "subtype": "success", "is_error": False, "result": content[0]["text"], "session_id": "fixture-session", "stop_reason": "end_turn", "terminal_reason": "completed", "num_turns": 1, "result_index": 0, "queued_turn_count": 0, "permission_denials": [], "usage": usage, "modelUsage": {"claude-fixture-1": {"inputTokens": 11, "outputTokens": 7, "cacheReadInputTokens": 3, "cacheCreationInputTokens": 2, "costUSD": 9999.99, "provider": "firstParty"}}, "total_cost_usd": 9999.99}
if mode == "unknown-usage":
    result.pop("usage"); result.pop("modelUsage")
if mode in ["paid-auth-error", "aggregate-paid-error"]:
    result.update(is_error=True, api_error_status=401, result="DO NOT ECHO SECRET")
if mode == "aborted":
    result["terminal_reason"] = "aborted_streaming"
if mode == "truncated":
    result["stop_reason"] = "max_tokens"
if mode == "wrong-session":
    result["session_id"] = "another-session"
if mode == "aggregate-conflict":
    result["modelUsage"]["claude-fixture-1"]["inputTokens"] = 2
if not result["is_error"]:
    if mode == "invalid":
        result["result"] = "not a JSON document"
    elif "MARKITAI_DOCUMENT_JSON_V1" in system or "MARKITAI_VISION_JSON_V1" in system:
        result["result"] = json.dumps({"cleaned_markdown": content[0]["text"], "frontmatter": {"description": "Authored subscription fixture.", "tags": ["fixture"]}}, ensure_ascii=False)
    elif content[0]["text"] == "Reply with exactly OK.":
        result["result"] = "OK"
emit(result)
if mode == "extra-result":
    emit(result)
else:
    emit({"type": "system", "subtype": "session_state_changed", "state": "idle", "session_id": "fixture-session"})
sys.exit(7 if mode == "bad-exit" else 0)
