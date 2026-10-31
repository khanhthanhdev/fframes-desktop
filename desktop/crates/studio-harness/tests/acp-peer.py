"""Protocol test peer; never used as a provider or qualification result.

Usage: acp-peer.py MODE ROOT. ROOT receives evidence files written by the peer
(permission.json, calls.json, helper pids, ...). Every mode drives the SDK-based
production driver.
"""
import json
import os
import pathlib
import subprocess
import sys
import time

mode = sys.argv[1]
root = pathlib.Path(sys.argv[2])
SID = "test-session"


def send(message):
    print(json.dumps({"jsonrpc": "2.0", **message}), flush=True)


def raw(text):
    sys.stdout.write(text)
    sys.stdout.flush()


def record(name, value):
    (root / name).write_text(json.dumps(value))


SESSION_ID = SID


def update(body, sid=None):
    send({"method": "session/update", "params": {"sessionId": sid or SESSION_ID, "update": body}})


def chunk(text, kind="agent_message_chunk", sid=None):
    update({"sessionUpdate": kind, "content": {"type": "text", "text": text}}, sid)


SECRET = os.environ.get("ACP_SECRET", "")
if mode == "meta-secret":
    SESSION_ID = "sess-" + SECRET
received = []
mcp_values = []
prompt_id = None
authed = False
helper = None

AUTH_MODES = ("auth-required", "auth-rejected", "auth-ok")
SECRET_OPTIONS = {
    "modes": {"currentModeId": "mode-" + SECRET, "availableModes": [{"id": "mode-" + SECRET, "name": "Code"}, {"id": "mode2-" + SECRET, "name": "Plan"}]},
    "configOptions": [{"id": "cfg-" + SECRET, "name": "Model", "type": "select", "currentValue": "val-" + SECRET, "options": [{"value": "val-" + SECRET, "name": "Fast"}, {"value": "val2-" + SECRET, "name": "Deep"}]}],
}
OPTIONS = {
    "modes": {"currentModeId": "code", "availableModes": [{"id": "code", "name": "Code"}, {"id": "plan", "name": "Plan"}]},
    "configOptions": [{"id": "model", "name": "Model", "type": "select", "currentValue": "fast", "options": [{"value": "fast", "name": "Fast"}, {"value": "deep", "name": "Deep"}]}],
}


def permission_request(rid="perm-1", title="Edit title"):
    call_id = "edit-title"
    allow, reject = "allow", "reject"
    if mode == "meta-secret":
        call_id, allow, reject = "call-" + SECRET, "allow-" + SECRET, "reject-" + SECRET
    send({"id": rid, "method": "session/request_permission", "params": {"sessionId": SESSION_ID, "toolCall": {"toolCallId": call_id, "title": title}, "options": [{"optionId": allow, "name": "Allow once", "kind": "allow_once"}, {"optionId": reject, "name": "Reject", "kind": "reject_once"}]}})


def finish(reason="end_turn"):
    """Answers the active prompt exactly once."""
    global prompt_id
    if prompt_id is None:
        return
    send({"id": prompt_id, "result": {"stopReason": reason}})
    prompt_id = None


def wait_for_file(name, timeout=20):
    deadline = time.time() + timeout
    while not (root / name).exists():
        if time.time() > deadline:
            sys.exit(9)
        time.sleep(0.005)


def spawn_helper(escape, hold_pipes=False):
    global helper
    kwargs = {}
    if escape:
        kwargs["start_new_session"] = True
    if not hold_pipes:
        kwargs.update(stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    helper = subprocess.Popen(["sleep", "60"], **kwargs)
    (root / "helper.pid").write_text(str(helper.pid))


turn_count = 0
first_prompt_id = None
line_bytes = 0
CLAUDE_NAME = "@agentclientprotocol/claude-agent-acp"
SERVER_TOOL_MESSAGE = "Internal error: Claude ended the turn without returning results for tool calls: "


def pulse(seconds, interval, bodies):
    """Sends the update bodies in turn every `interval` seconds for `seconds`."""
    deadline = time.time() + seconds
    i = 0
    while time.time() < deadline:
        update(bodies[i % len(bodies)])
        i += 1
        time.sleep(interval)


def run_prompt(message):
    global prompt_id, turn_count, first_prompt_id
    prompt_id = message["id"]
    turn_count += 1
    text = message["params"]["prompt"][0]["text"]
    images = sum(1 for block in message["params"]["prompt"] if block.get("type") == "image")
    record("prompt.json", {"sessionId": message["params"]["sessionId"], "text": text, "lineBytes": line_bytes, "images": images})
    if mode == "secret-tail":
        chunk("tails" if turn_count == 1 else "Hello")
        finish()
        return
    if mode == "stale-finish":
        if turn_count == 1:
            first_prompt_id = prompt_id
            chunk("first")
            finish()
            return
        send({"id": first_prompt_id, "result": {"stopReason": "end_turn"}})
        # Dispatched after the stale response: once the host sees it, the stale one was handled.
        update({"sessionUpdate": "tool_call", "toolCallId": "after-stale", "title": "Marker", "kind": "other", "status": "pending"})
        record("stale-sent", True)
        wait_for_file("go")
        chunk("second")
        finish()
        return
    if mode == "thoughts":
        chunk("plan A", "agent_thought_chunk")
        chunk("Answer ")
        chunk("more thinking", "agent_thought_chunk")
        chunk("done.")
        update({"sessionUpdate": "agent_message_chunk", "content": {"type": "image", "data": "AAAA", "mimeType": "image/png"}})
        finish()
        return
    if mode == "inert-updates":
        update({"sessionUpdate": "plan", "entries": [{"content": "step", "priority": "low", "status": "pending"}]})
        update({"sessionUpdate": "available_commands_update", "availableCommands": [{"name": "compact", "description": "Compact the context"}]})
        update({"sessionUpdate": "session_info_update", "title": "Renamed"})
        chunk("still fine")
        finish()
        return
    if mode in ("steer", "steer-reject"):
        chunk("working ")
        record("turn-open.json", {})
        return
    if mode in ("files", "embedded"):
        record("prompt-blocks.json", message["params"]["prompt"])
        chunk("seen")
        finish()
        return
    if mode == "plan":
        update({"sessionUpdate": "available_commands_update", "availableCommands": [
            {"name": "/compact", "description": "Compact the context"},
            {"name": "review", "description": "Review changes", "input": {"hint": "what to review"}},
        ]})
        update({"sessionUpdate": "plan", "entries": [
            {"content": "read the code", "priority": "high", "status": "completed"},
            {"content": "edit it", "priority": "medium", "status": "in_progress"},
            {"content": "test it", "priority": "low", "status": "pending"},
        ]})
        chunk("planned")
        finish()
        return
    if mode == "push-options":
        update({"sessionUpdate": "current_mode_update", "currentModeId": "plan"})
        update({"sessionUpdate": "config_option_update", "configOptions": [dict(OPTIONS["configOptions"][0], currentValue="deep")]})
        finish()
        return
    if mode == "empty-turn":
        finish()
        return
    if mode == "prompt-error":
        send({"id": prompt_id, "error": {"code": -32603, "message": "backend unavailable"}})
        prompt_id = None
        return
    if mode == "dup-finish":
        chunk("once")
        send({"id": prompt_id, "result": {"stopReason": "end_turn"}})
        send({"id": prompt_id, "result": {"stopReason": "end_turn"}})
        # Late, and dispatched after the duplicate: once it is counted, the duplicate was handled.
        update({"sessionUpdate": "tool_call", "toolCallId": "after-dup", "title": "Marker", "kind": "other", "status": "pending"})
        prompt_id = None
        return
    if mode == "foreign-perm":
        send({"id": "perm-1", "method": "session/request_permission", "params": {"sessionId": "someone-else", "toolCall": {"toolCallId": "x", "title": "Foreign"}, "options": [{"optionId": "allow", "name": "Allow once", "kind": "allow_once"}]}})
        return
    if mode == "unknown-method":
        send({"id": "x1", "method": "custom/ping", "params": {}})
        chunk("survived")
        finish()
        return
    if mode == "crlf":
        stdout = sys.stdout.buffer
        stdout.write(b"\r\n\n")
        for text in ("crlf ", "framed"):
            body = json.dumps({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": SESSION_ID, "update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text}}}})
            stdout.write(body.encode() + b"\r\n")
        stdout.flush()
        finish()
        return
    if mode == "race-end-turn":
        wait_for_file("go")
        finish()
        return
    if mode == "perm-after-cancel":
        wait_for_file("go")
        permission_request()
        return
    if mode == "tools-n":
        for i in range(int(text)):
            update({"sessionUpdate": "tool_call", "toolCallId": f"t{i}", "title": f"Tool {i}", "kind": "other", "status": "pending"})
        finish()
        return
    if mode == "dup-perm-id":
        permission_request("dup", "First")
        permission_request("dup", "Second")
        return
    if mode == "perm-full":
        update({"sessionUpdate": "tool_call", "toolCallId": "t0", "title": "Tool", "kind": "other", "status": "pending"})
        permission_request()
        return
    if mode == "meta-secret":
        update({"sessionUpdate": "tool_call", "toolCallId": "call-" + SECRET, "title": "tool", "kind": "other", "status": "pending"})
        permission_request()
        return
    if mode == "secret-whole":
        chunk("whole secret " + SECRET + " inside one wire line. ")
        update({"sessionUpdate": "tool_call", "toolCallId": "s1", "title": "use " + SECRET, "kind": "other", "status": "pending"})
        finish()
        return
    if mode == "flood-requests":
        for i in range(3000):
            send({"id": f"fs-{i}", "method": "fs/read_text_file", "params": {"sessionId": SESSION_ID, "path": "/etc/hostname"}})
        time.sleep(120)
        return
    if mode == "flood-plans":
        entry = {"content": "step", "priority": "low", "status": "pending"}
        for i in range(50000):
            update({"sessionUpdate": "plan", "entries": [entry]})
        finish()
        return
    if mode == "stderr-fragments":
        half = len(SECRET) // 2
        sys.stderr.write("adapter says hello " + SECRET[:half])
        sys.stderr.flush()
        record("frag1", True)
        wait_for_file("ack1")
        sys.stderr.write(SECRET[half:] + " and then bye\n")
        sys.stderr.flush()
        record("frag2", True)
        wait_for_file("ack2")
        finish()
        return
    if mode == "big-delta":
        chunk("a" * 900000 + " tail")
        finish()
        return
    if mode.startswith("srvtool"):
        if turn_count > 1:
            finish()
            return
        ids = ["srvtoolu_a", "toolu_b"] if mode == "srvtool-client" else ["srvtoolu_a", "srvtoolu_b"]
        # Like the Claude adapter: fail every unfinished call, then report the turn.
        for tool_id in ids:
            update({"sessionUpdate": "tool_call_update", "toolCallId": tool_id, "status": "failed"})
        error = {"code": -32603, "message": SERVER_TOOL_MESSAGE + ", ".join(ids)}
        if mode != "srvtool-nodata":
            error["data"] = {"errorKind": "incomplete_tool_call"}
        send({"id": prompt_id, "error": error})
        prompt_id = None
        return
    if mode == "inert-forever":
        pulse(30, 0.1, [
            {"sessionUpdate": "available_commands_update", "availableCommands": [{"name": "compact", "description": "Compact"}]},
            {"sessionUpdate": "session_info_update", "title": "Busy"},
        ])
        return
    if mode == "active-slow":
        pulse(2, 0.1, [
            {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "working "}},
            {"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "thinking "}},
            {"sessionUpdate": "plan", "entries": [{"content": "step", "priority": "low", "status": "pending"}]},
            {"sessionUpdate": "tool_call", "toolCallId": "slow", "title": "Slow tool", "kind": "other", "status": "in_progress"},
            {"sessionUpdate": "tool_call_update", "toolCallId": "slow", "status": "in_progress"},
        ])
        finish()
        return
    if mode == "active-forever":
        pulse(30, 0.1, [{"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "still going "}}])
        return
    if mode == "hang-prompt" or mode == "hang-cancel" or mode == "stall-stdin":
        if mode == "stall-stdin":
            time.sleep(120)
        return
    if mode == "permission":
        chunk("Needs approval. ")
        permission_request()
        return
    if mode == "permission-two":
        permission_request("perm-1")
        permission_request("perm-2", "Second")
        return
    if mode == "refuse":
        chunk("I will not do that.")
        finish("refusal")
        return
    if mode == "max-tokens":
        finish("max_tokens")
        return
    if mode == "unknown-stop":
        finish("bogus")
        return
    if mode == "wrong-session":
        chunk("oops", sid="someone-else")
        return
    if mode == "malformed-mid":
        raw("{not json}\n")
        return
    if mode == "oversized-mid":
        raw("y" * (2 * 1024 * 1024) + "\n")
        return
    if mode == "truncated-mid":
        raw('{"jsonrpc":"2.0","method":"session/upd')
        sys.exit(0)
    if mode == "crash-mid":
        chunk("about to crash")
        sys.exit(7)
    if mode == "fs-request":
        send({"id": "fs-1", "method": "fs/read_text_file", "params": {"sessionId": SID, "path": "/etc/hostname"}})
        send({"id": "term-1", "method": "terminal/create", "params": {"sessionId": SID, "command": "ls"}})
        return
    if mode == "flood-tools":
        for i in range(2000):
            update({"sessionUpdate": "tool_call", "toolCallId": f"t{i}", "title": f"Tool {i}", "kind": "other", "status": "pending"})
        finish()
        return
    if mode == "flood-deltas":
        for i in range(20000):
            chunk(f"delta-{i} ")
        finish()
        return
    if mode == "mcp-secret":
        value = mcp_values[0]
        sys.stderr.write(f"mcp debug key={value} token={value}\n")
        sys.stderr.flush()
        half = len(value) // 2
        chunk("The mcp key is ")
        chunk(value[:half])
        chunk(value[half:])
        chunk(" done")
        update({"sessionUpdate": "tool_call", "toolCallId": "m1", "title": f"use {value}", "kind": "other", "status": "pending"})
        finish()
        return
    if mode == "secret":
        sys.stderr.write(f"adapter debug key={SECRET} token={SECRET}\n")
        sys.stderr.flush()
        half = len(SECRET) // 2
        chunk("The key is ")
        chunk(SECRET[:half])
        chunk(SECRET[half:])
        chunk(" done")
        update({"sessionUpdate": "tool_call", "toolCallId": "s1", "title": f"use {SECRET}", "kind": "other", "status": "pending"})
        finish()
        return
    if mode == "spawn-helper":
        spawn_helper(False)
    if mode == "escape-helper":
        spawn_helper(True)
    if mode == "escape-pipes":
        spawn_helper(True, hold_pipes=True)
    if mode == "late":
        chunk("before ")
        update({"sessionUpdate": "tool_call", "toolCallId": "a", "title": "Edit", "kind": "edit", "status": "in_progress"})
        update({"sessionUpdate": "tool_call_update", "toolCallId": "a", "status": "completed"})
        finish()
        # Everything below arrives after the authoritative completion.
        chunk("late text")
        update({"sessionUpdate": "tool_call", "toolCallId": "late", "title": "Late tool", "kind": "edit", "status": "pending"})
        permission_request("late-perm", "Late request")
        return
    chunk("Hello ")
    chunk("world")
    update({"sessionUpdate": "tool_call", "toolCallId": "call-1", "title": "Edit file", "kind": "edit", "status": "in_progress"})
    update({"sessionUpdate": "tool_call_update", "toolCallId": "call-1", "status": "completed"})
    if mode == "write":
        (root / "main.rs").write_text('fn main() { println!("agent edit"); }')
    finish()


for line in sys.stdin:
    line_bytes = len(line.rstrip("\r\n").encode("utf-8"))
    message = json.loads(line)
    method = message.get("method")
    if method:
        received.append(method)
        record("calls.json", received)
    if method == "initialize":
        params = message["params"]
        record("initialize.json", params)
        if mode == "hang-init":
            time.sleep(120)
        if mode == "wedge-init":
            # An escaped helper keeps the adapter's stdout/stderr open, then init hangs.
            spawn_helper(True, hold_pipes=True)
            time.sleep(120)
        if mode == "exit-early":
            sys.exit(3)
        if mode == "garbage-init":
            raw("this is not json\n")
            continue
        if mode == "truncated":
            raw('{"jsonrpc":"2.0","id":')
            sys.exit(0)
        if mode == "oversized":
            raw("x" * (2 * 1024 * 1024) + "\n")
            continue
        version = 2 if mode == "version2" else 1
        result = {
            "protocolVersion": version,
            "agentInfo": {"name": "protocol-peer", "version": "test"},
            "agentCapabilities": {"loadSession": mode == "load", "promptCapabilities": {"image": mode == "image-ok", "embeddedContext": mode == "embedded"}},
            "authMethods": [{"id": "key", "name": "API key"}] if mode in AUTH_MODES else [],
        }
        if mode in ("steer", "steer-reject"):
            result["agentCapabilities"]["_meta"] = {"steering": True}
        if mode == "mcp-http":
            result["agentCapabilities"]["mcpCapabilities"] = {"http": True}
        if mode == "meta-secret":
            result["agentInfo"] = {"name": "agent-" + SECRET, "version": "v-" + SECRET}
        if mode.startswith("srvtool") and mode != "srvtool-other-agent":
            result["agentInfo"] = {"name": CLAUDE_NAME, "version": "test"}
        send({"id": message["id"], "result": result})
    elif method == "authenticate":
        if mode == "auth-rejected":
            send({"id": message["id"], "error": {"code": -32603, "message": f"credentials rejected {SECRET}"}})
        else:
            authed = True
            send({"id": message["id"], "result": {}})
    elif method in ("session/new", "session/load"):
        assert pathlib.Path(message["params"]["cwd"]).is_absolute()
        record("session-params.json", {"method": method, **message["params"]})
        mcp_values = [e["value"] for server in message["params"].get("mcpServers", []) for e in server.get("env", [])]
        if mode == "mcp-reject":
            send({"id": message["id"], "error": {"code": -32603, "message": "session refused: " + ",".join(mcp_values)}})
        elif mode in ("auth-required",) or (mode == "auth-ok" and not authed):
            send({"id": message["id"], "error": {"code": -32000, "message": "Authentication required"}})
        elif method == "session/load":
            SESSION_ID = message["params"]["sessionId"]
            chunk("replayed question", "user_message_chunk", sid=message["params"]["sessionId"])
            chunk("replayed answer", "agent_message_chunk", sid=message["params"]["sessionId"])
            update({"sessionUpdate": "available_commands_update", "availableCommands": [{"name": "compact", "description": "Compact the context"}]}, message["params"]["sessionId"])
            update({"sessionUpdate": "plan", "entries": [{"content": "resume work", "priority": "high", "status": "in_progress"}]}, message["params"]["sessionId"])
            send({"id": message["id"], "result": {}})
        else:
            result = {"sessionId": SESSION_ID}
            if mode in ("options", "options-silent", "push-options"):
                result.update(OPTIONS)
            if mode == "meta-secret":
                result.update(SECRET_OPTIONS)
            send({"id": message["id"], "result": result})
            if mode == "idle-crash":
                sys.exit(5)
    elif method == "session/set_mode":
        record("set-mode.json", message["params"])
        if mode == "options-silent":
            pass
        elif mode in ("options", "meta-secret"):
            send({"id": message["id"], "result": {}})
        else:
            send({"id": message["id"], "error": {"code": -32602, "message": "mode not supported"}})
    elif method == "session/set_config_option":
        record("set-config.json", message["params"])
        if mode == "options-silent":
            pass
        elif mode == "meta-secret":
            cfg = [dict(SECRET_OPTIONS["configOptions"][0], currentValue=message["params"]["value"])]
            send({"id": message["id"], "result": {"configOptions": cfg}})
        elif message["params"].get("value") == "deep":
            cfg = [dict(OPTIONS["configOptions"][0], currentValue="deep")]
            send({"id": message["id"], "result": {"configOptions": cfg}})
        else:
            send({"id": message["id"], "error": {"code": -32602, "message": "rejected"}})
    elif method == "session/prompt":
        run_prompt(message)
    elif method == "_session/steering":
        record("steering.json", message["params"])
        if mode == "steer-reject":
            send({"id": message["id"], "error": {"code": -32602, "message": "cannot steer now"}})
        else:
            send({"id": message["id"], "result": {}})
            chunk("steered")
            finish()
    elif method == "session/cancel":
        record("cancel.json", message)
        if mode == "hang-cancel":
            continue
        finish("cancelled")
    elif message.get("id") == "dup" and "result" in message:
        record("permission-dup.json", message["result"]["outcome"])
    elif message.get("id") in ("perm-1", "perm-2") and "result" in message:
        outcome = message["result"]["outcome"]
        record(f"permission-{message['id']}.json", outcome)
        if mode in ("perm-after-cancel", "perm-full", "meta-secret"):
            finish("cancelled" if outcome["outcome"] == "cancelled" else "end_turn")
        elif mode == "permission" and outcome["outcome"] == "selected":
            if outcome["optionId"] == "allow":
                (root / "main.rs").write_text('fn main() { println!("Permitted edit"); }')
                chunk("Applied.")
            else:
                chunk("Rejected.")
            finish()
        elif mode == "permission" and outcome["outcome"] == "cancelled":
            finish("cancelled")
    elif message.get("id") == "x1":
        record("custom-ping.json", message)
    elif message.get("id") == "late-perm":
        record("late-perm.json", message.get("result"))
    elif message.get("id") in ("fs-1", "term-1"):
        record(f"{message['id']}.json", message)
        if message["id"] == "term-1":
            finish()
