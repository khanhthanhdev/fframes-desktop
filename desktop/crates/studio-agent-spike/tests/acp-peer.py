"""Protocol test peer; never used as a provider or qualification result.

Usage: acp-peer.py MODE ROOT. ROOT receives evidence files written by the peer
(permission.json, calls.json, helper pids, ...). Legacy modes (edit, question,
bad-stop, blocked) drive the handwritten spike session; every other mode drives the
SDK-based production driver.
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


def legacy():
    if mode == "blocked":
        time.sleep(30)
        sys.exit()

    prompt_id = None
    initializations = sessions = 0
    turns = []
    for line in sys.stdin:
        message = json.loads(line)
        method = message.get("method")
        if method == "initialize":
            initializations += 1
            send({"id": message["id"], "result": {"protocolVersion": 1, "agentInfo": {"name": "protocol-peer", "version": "test"}}})
        elif method == "session/new":
            sessions += 1
            assert pathlib.Path(message["params"]["cwd"]).is_absolute()
            (root / "target").mkdir(exist_ok=True)
            (root / "target/session-new.json").write_text(json.dumps(message["params"]))
            send({"id": message["id"], "result": {"sessionId": "test-session"}})
        elif method == "session/prompt":
            prompt_id = message["id"]
            if mode == "question":
                assert message["params"]["sessionId"] == "test-session"
                turns.append(message["params"]["prompt"][0]["text"])
                (root / "target").mkdir(exist_ok=True)
                (root / "target/turns.json").write_text(json.dumps({"initializations": initializations, "sessions": sessions, "prompts": turns}))
                if len(turns) == 1:
                    text = "Which title should I use?"
                else:
                    text = turns[-1]
                    (root / "main.rs").write_text(text)
                send({"method": "session/update", "params": {"sessionId": "test-session", "update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text}}}})
                send({"id": prompt_id, "result": {"stopReason": "end_turn"}})
                continue
            sys.stderr.write("x" * 200000 + " token=split-secret\n")
            sys.stderr.flush()
            for text in ["Streaming ", "split-", "secret", " response"]:
                send({"method": "session/update", "params": {"sessionId": "test-session", "update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text}}}})
            send({"id": "permission-1", "method": "session/request_permission", "params": {"sessionId": "test-session", "toolCall": {"toolCallId": "edit-title", "title": "Edit title"}, "options": [{"optionId": "allow", "name": "Allow once", "kind": "allow_once"}, {"optionId": "reject", "name": "Reject", "kind": "reject_once"}]}})
        elif message.get("id") == "permission-1":
            outcome = message["result"]["outcome"]
            (root / "permission.json").write_text(json.dumps(outcome))
            if outcome["outcome"] == "selected":
                assert outcome["optionId"] == "allow"
                (root / "main.rs").write_text('fn main() { println!("Edited title"); }')
                send({"id": prompt_id, "result": {"stopReason": "split-secret" if mode == "bad-stop" else "end_turn"}})
        elif method == "session/cancel":
            (root / "cancel.json").write_text(json.dumps(message))
            send({"id": prompt_id, "result": {"stopReason": "cancelled"}})


if mode in ("edit", "question", "bad-stop", "blocked"):
    legacy()
    sys.exit()

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


def run_prompt(message):
    global prompt_id
    prompt_id = message["id"]
    text = message["params"]["prompt"][0]["text"]
    record("prompt.json", {"sessionId": message["params"]["sessionId"], "text": text})
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
            "agentCapabilities": {"loadSession": mode == "load", "promptCapabilities": {"image": False}},
            "authMethods": [{"id": "key", "name": "API key"}] if mode in AUTH_MODES else [],
        }
        if mode == "meta-secret":
            result["agentInfo"] = {"name": "agent-" + SECRET, "version": "v-" + SECRET}
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
            chunk("replayed question", "user_message_chunk", sid=message["params"]["sessionId"])
            chunk("replayed answer", "agent_message_chunk", sid=message["params"]["sessionId"])
            send({"id": message["id"], "result": {}})
        else:
            result = {"sessionId": SESSION_ID}
            if mode in ("options", "options-silent"):
                result.update(OPTIONS)
            if mode == "meta-secret":
                result.update(SECRET_OPTIONS)
            send({"id": message["id"], "result": result})
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
            (root / "main.rs").write_text('fn main() { println!("Permitted edit"); }')
            chunk("Applied.")
            finish()
        elif mode == "permission" and outcome["outcome"] == "cancelled":
            finish("cancelled")
    elif message.get("id") == "late-perm":
        record("late-perm.json", message.get("result"))
    elif message.get("id") in ("fs-1", "term-1"):
        record(f"{message['id']}.json", message)
        if message["id"] == "term-1":
            finish()
