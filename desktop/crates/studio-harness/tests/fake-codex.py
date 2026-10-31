"""Fake `codex app-server` speaking newline-delimited JSON-RPC.

Message shapes follow the transcripts recorded from codex-cli 0.162.1 and the schema
`codex app-server generate-json-schema` writes. It proves the driver's framing,
handshake, approvals, turns, background commands and stop only.

Usage: fake-codex.py <mode> <root> [codex args...]
Modes: turns, old, signed-out, no-init. Whole uppercase words in the prompt pick the
behaviour of each turn (RUN, EDIT, ASK, PLAN, SLOW, BACKGROUND [HOLD], STEER, FAIL,
UNAUTH, CHILD, TOOLCALL, QUIET); anything else answers with text. `/compact` and
`/review` run as their own requests (SLOW in review instructions waits for an interrupt). Observations go to
<root>/*.json; every turn writes <root>/writes.json ({path: text}) into the working
directory, and a background command writes <root>/late-writes.json when it ends.
"""

import json
import os
import queue
import re
import sys
import threading
import time
import uuid

MODE = sys.argv[1] if len(sys.argv) > 1 else "turns"
ROOT = sys.argv[2] if len(sys.argv) > 2 else "."
ARGS = sys.argv[3:]
# Values the driver must never surface: a user MCP server's secret and the account email.
SENTINEL = "codex-user-mcp-sentinel-77e1"
EMAIL = "person-sentinel@example.com"

if "--version" in ARGS:
    print("codex-cli 0.150.0" if MODE == "old" else "codex-cli 0.162.1")
    sys.exit(0)


def record(name, value):
    with open(os.path.join(ROOT, name), "w", encoding="utf-8", newline="\n") as f:
        json.dump(value, f)


record("launch.json", {"args": ARGS, "cwd": os.getcwd()})
THREAD = None
log = []
queued = []
# The running turn: its id once announced, its announcement time, and "interrupted".
interrupt = {"turn": None, "started_at": None}
inbox = queue.Queue()


def pump():
    """Stamps every line with its arrival time, so an early interrupt is told apart."""
    for line in sys.stdin:
        inbox.put((time.time(), line))
    inbox.put((time.time(), None))


threading.Thread(target=pump, daemon=True).start()


def send(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def notify(method, **params):
    send({"method": method, "params": params})


def read():
    if queued:
        return queued.pop(0)
    arrived, line = inbox.get()
    if not line:
        record("log.json", log)
        os._exit(0)
    message = json.loads(line)
    log.append(dict(message))
    record("log.json", log)
    message["_arrived"] = arrived
    return message


next_request = [0]


def server_request(method, params):
    """Sends a request and returns the client's response, queueing anything else."""
    request_id = next_request[0]
    next_request[0] += 1
    send({"id": request_id, "method": method, "params": params})
    while True:
        message = read()
        if message.get("id") == request_id and "method" not in message:
            notify("serverRequest/resolved", threadId=THREAD, requestId=request_id)
            return message
        handle_side(message)


def handle_side(message):
    """A client message that arrives while a turn waits: interrupts are answered here."""
    if message.get("method") == "turn/interrupt":
        started_at = interrupt["started_at"]
        if started_at is None or message["_arrived"] < started_at:
            send({"id": message["id"], "error": {"code": -32600, "message": "no active turn to interrupt"}})
            record("early-interrupt.json", True)
        elif message["params"].get("turnId") != interrupt["turn"]:
            send({"id": message["id"], "error": {"code": -32600, "message": "turn id does not match"}})
            record("wrong-interrupt.json", message["params"])
        else:
            send({"id": message["id"], "result": {}})
            interrupt["turn"] = "interrupted"
    else:
        queued.append(message)


def item(turn, kind, item_id, **fields):
    return dict(type=kind, id=item_id, **fields)


def started(turn, value):
    notify("item/started", item=value, threadId=THREAD, turnId=turn, startedAtMs=0)


def completed(turn, value):
    notify("item/completed", item=value, threadId=THREAD, turnId=turn, completedAtMs=0)


def turn_object(turn, status, error=None):
    return {"id": turn, "items": [], "itemsView": "notLoaded", "status": status, "error": error}


def write_files(name):
    path = os.path.join(ROOT, name)
    if not os.path.exists(path):
        return
    with open(path, encoding="utf-8") as f:
        files = json.load(f)
    for relative, contents in files.items():
        target = os.path.join(os.getcwd(), relative)
        os.makedirs(os.path.dirname(target), exist_ok=True)
        with open(target, "w", encoding="utf-8", newline="\n") as f:
            f.write(contents)


def approval(turn, method, item_id, **fields):
    params = dict(threadId=fields.pop("thread", THREAD), turnId=turn, itemId=item_id, startedAtMs=0, **fields)
    return server_request(method, params)["result"]


mcp_turns = [0]


def run_turn(request):
    params = request["params"]
    turn = "turn-" + str(uuid.uuid4())
    send({"id": request["id"], "result": {"turn": turn_object(turn, "inProgress")}})
    text_parts = [i.get("text", "") for i in params["input"] if i.get("type") == "text"]
    record("images.json", [i["path"] for i in params["input"] if i.get("type") == "localImage"])
    record("turn.json", params)
    words = set(re.findall(r"\b[A-Z]+\b", " ".join(text_parts)))
    # NOSTART: the turn is announced only once the test creates the `release` file.
    while "NOSTART" in words and not os.path.exists(os.path.join(ROOT, "release")):
        time.sleep(0.05)
    # The server announces the turn a little after answering turn/start.
    deadline = time.time() + 0.3
    while time.time() < deadline:
        time.sleep(0.02)
    interrupt["turn"] = turn
    interrupt["started_at"] = time.time()
    notify("turn/started", threadId=THREAD, turn=turn_object(turn, "inProgress"))
    if "UNAUTH" in words:
        notify("turn/completed", threadId=THREAD, turn=turn_object(turn, "failed", {
            "message": "unauthorized", "codexErrorInfo": "unauthorized"}))
        return
    notify("item/reasoning/summaryTextDelta", threadId=THREAD, turnId=turn, itemId="rs1", delta="thinking", summaryIndex=0)
    if "QUIET" not in words:
        notify("item/agentMessage/delta", threadId=THREAD, turnId=turn, itemId="msg1", delta="on it")
        completed(turn, item(turn, "agentMessage", "msg1", text="on it"))
    else:
        completed(turn, item(turn, "agentMessage", "msg1", text="whole message"))
    write_files("writes.json")
    if "RUN" in words:
        command = item(turn, "commandExecution", "exec-1", command="echo ran > ran.txt", cwd=os.getcwd(),
                       status="inProgress", commandActions=[], source="agent")
        started(turn, command)
        result = approval(turn, "item/commandExecution/requestApproval", "exec-1", command="echo ran > ran.txt",
                          cwd=os.getcwd(), availableDecisions=["accept", "cancel"])
        record("decision.json", result)
        accepted = result.get("decision") == "accept"
        if accepted:
            with open(os.path.join(os.getcwd(), "ran.txt"), "w", encoding="utf-8", newline="\n") as f:
                f.write("ran\n")
        completed(turn, dict(command, status="completed" if accepted else "declined", exitCode=0 if accepted else None))
    if "EDIT" in words:
        target = os.path.join(os.getcwd(), "hello.txt")
        change = item(turn, "fileChange", "patch-1", changes=[{"path": target, "kind": {"type": "update"}, "diff": "-hello\n+allowed\n"}],
                      status="inProgress")
        started(turn, change)
        result = approval(turn, "item/fileChange/requestApproval", "patch-1", reason=None, grantRoot=None)
        record("decision.json", result)
        accepted = result.get("decision") == "accept"
        if accepted:
            with open(target, "w", encoding="utf-8", newline="\n") as f:
                f.write("allowed\n")
        completed(turn, dict(change, status="completed" if accepted else "declined"))
    if "CHILD" in words:
        result = approval(turn, "item/commandExecution/requestApproval", "exec-child", thread="child-thread",
                          command="ls")
        record("child-decision.json", result)
    if "MCPTOOL" in words:
        # Codex 0.163 asks before an MCP tool runs (approval policy `untrusted`, no
        # read-only hint): Studio's own server first, then the user's `docs` server, which
        # runs as `studio-docs` because config.toml has a `docs`.
        # codex-rs main names the tool in `_meta.tool_name`; 0.163 only in the message.
        mcp_turns[0] += 1
        for server, tool, name in (("studio", "project_context", "studio-approval.json"),
                                   ("studio-docs", "search", "mcp-approval.json")):
            meta = {"codex_approval_kind": "mcp_tool_call", "persist": ["session", "always"]}
            if mcp_turns[0] % 2:
                meta["tool_name"] = tool
            reply = server_request("mcpServer/elicitation/request", {
                "threadId": THREAD, "turnId": turn, "serverName": server, "mode": "form",
                "_meta": meta,
                "message": "Allow the %s MCP server to run tool \"%s\"?" % (server, tool),
                "requestedSchema": {"type": "object", "properties": {}}})
            record(name, reply)
    if "TOOLCALL" in words:
        record("toolcall.json", server_request("item/tool/call", {"threadId": THREAD, "turnId": turn, "tool": "x"}))
    if "ASK" in words:
        result = server_request("item/tool/requestUserInput", {
            "threadId": THREAD, "turnId": turn, "itemId": "ask-1", "isBlocking": True, "questions": [
                {"id": "color", "header": "Color", "question": "Which color?",
                 "options": [{"label": "Red", "description": "warm"}, {"label": "Blue", "description": ""}]},
                {"id": "name", "header": "", "question": "Your name?", "options": None, "isOther": True},
            ]})
        record("answers.json", result["result"])
    if "PLAN" in words:
        notify("turn/plan/updated", threadId=THREAD, turnId=turn, explanation=None, plan=[
            {"step": "First", "status": "completed"}, {"step": "Second", "status": "inProgress"}])
    if "STEER" in words:
        while True:
            message = read()
            if message.get("method") == "turn/steer":
                break
            handle_side(message)
        if MODE == "steer-reject":
            send({"id": message["id"], "error": {"code": -32600, "message": "expected turn mismatch"}})
        else:
            record("steer.json", message["params"])
            send({"id": message["id"], "result": {"turnId": turn}})
            completed(turn, item(turn, "userMessage", "user-2", content=message["params"]["input"]))
            notify("item/agentMessage/delta", threadId=THREAD, turnId=turn, itemId="msg2", delta="steered")
    if "SLOW" in words:
        while interrupt["turn"] != "interrupted":
            handle_side(read())
            while queued and queued[0].get("method") != "turn/interrupt":
                queued.pop(0)
        notify("turn/completed", threadId=THREAD, turn=turn_object(turn, "interrupted"))
        interrupt.update(turn=None, started_at=None)
        return
    if "FAIL" in words:
        notify("turn/completed", threadId=THREAD, turn=turn_object(turn, "failed", {
            "message": "usage limit", "codexErrorInfo": "usageLimitExceeded"}))
        interrupt.update(turn=None, started_at=None)
        return
    if "BACKGROUND" in words:
        command = item(turn, "commandExecution", "exec-bg", command="sleep", cwd=os.getcwd(),
                       status="inProgress", commandActions=[], source="unifiedExecStartup")
        started(turn, command)
        notify("turn/completed", threadId=THREAD, turn=turn_object(turn, "completed"))
        interrupt.update(turn=None, started_at=None)
        time.sleep(0.5)
        while "HOLD" in words and not os.path.exists(os.path.join(ROOT, "release")):
            time.sleep(0.05)
        write_files("late-writes.json")
        completed(turn, dict(command, status="completed", exitCode=0))
        record("background-done.json", True)
        return
    notify("thread/tokenUsage/updated", threadId=THREAD, turnId=turn, tokenUsage={
        "total": {"totalTokens": 3}, "last": {"totalTokens": 3}})
    notify("turn/completed", threadId=THREAD, turn=turn_object(turn, "completed"))
    interrupt.update(turn=None, started_at=None)


def run_command_turn(request, kind):
    """`/compact` replies {} and announces its turn; `/review` replies with one turn id
    and announces another, then completes under the reply's id (as 0.162.1 does)."""
    record(kind + ".json", request["params"])
    instructions = request["params"].get("target", {}).get("instructions", "")
    if "REFUSE" in instructions:
        send({"id": request["id"], "error": {"code": -32600, "message": "nothing to review"}})
        return
    reply = "turn-" + str(uuid.uuid4())
    announced = reply if kind == "compact" else "turn-" + str(uuid.uuid4())
    send({"id": request["id"], "result": {} if kind == "compact" else {
        "reviewThreadId": THREAD, "turn": turn_object(reply, "inProgress")}})
    time.sleep(0.1)
    interrupt["turn"] = announced
    interrupt["started_at"] = time.time()
    notify("turn/started", threadId=THREAD, turn=turn_object(announced, "inProgress"))
    if "SLOW" in instructions:
        while interrupt["turn"] != "interrupted":
            handle_side(read())
            while queued and queued[0].get("method") != "turn/interrupt":
                queued.pop(0)
        status = "interrupted"
    else:
        item_kind = "contextCompaction" if kind == "compact" else "agentMessage"
        completed(announced, item(announced, item_kind, "cmd-item", text="reviewed"))
        if kind == "review":
            notify("item/agentMessage/delta", threadId=THREAD, turnId=announced, itemId="rv1", delta="reviewed")
        status = "completed"
    notify("turn/completed", threadId=THREAD, turn=turn_object(reply, status))
    interrupt.update(turn=None, started_at=None)


# The user's config.toml servers, as config/read reports them.
CONFIGURED = {
    "rea": {"command": "rea", "env": {"REA_TOKEN": SENTINEL}},
    "docs": {"command": "docs-cli", "args": ["--stdio"]},
}


def invalid_server(overrides):
    """Codex's check of the merged servers: thread config keys are one more layer merged
    key by key into config.toml's tables (codex-rs merge_toml_values), and an entry
    with both stdio and HTTP keys is refused (RawMcpServerConfig -> McpServerConfig)."""
    servers = json.loads(json.dumps(CONFIGURED))
    for key, value in overrides.items():
        parts = key.split(".")
        if parts[0] != "mcp_servers" or len(parts) != 3:
            continue
        entry = servers.setdefault(parts[1], {})
        if isinstance(value, dict) and isinstance(entry.get(parts[2]), dict):
            entry[parts[2]].update(value)
        else:
            entry[parts[2]] = value
    for name, entry in servers.items():
        if "command" in entry:
            for field in ("url", "bearer_token_env_var", "bearer_token", "http_headers", "env_http_headers"):
                if field in entry:
                    return "mcp_servers.%s: %s is not supported for stdio" % (name, field)
        elif "url" in entry:
            for field in ("args", "env", "env_vars", "cwd", "bearer_token"):
                if field in entry:
                    return "mcp_servers.%s: %s is not supported for streamable_http" % (name, field)
    return None


while True:
    message = read()
    method = message.get("method")
    if method == "initialize":
        if MODE == "no-init":
            continue
        send({"id": message["id"], "result": {"userAgent": "fframes-studio/0.162.1 (Windows; x86_64)",
              "codexHome": "/home/.codex", "platformFamily": "windows", "platformOs": "windows"}})
    elif method == "account/read":
        account = None if MODE == "signed-out" else {"type": "chatgpt", "email": EMAIL, "planType": "plus"}
        send({"id": message["id"], "result": {"account": account, "requiresOpenaiAuth": True}})
    elif method == "config/read":
        record("config-read.json", message["params"])
        send({"id": message["id"], "result": {"config": {"mcp_servers": CONFIGURED}, "origins": {}}})
    elif method in ("thread/start", "thread/resume"):
        record("thread.json", {"method": method, "params": message["params"]})
        refused = invalid_server(message["params"].get("config") or {})
        if refused:
            send({"id": message["id"], "error": {"code": -32600, "message": refused}})
            continue
        requested = message["params"].get("threadId")
        if method == "thread/resume" and requested == "gone":
            send({"id": message["id"], "error": {"code": -32600, "message": "thread not found"}})
            continue
        THREAD = requested or "thr-" + str(uuid.uuid4())
        send({"id": message["id"], "result": {"thread": {"id": THREAD, "status": {"type": "idle"}, "turns": []},
              "approvalPolicy": message["params"].get("approvalPolicy"), "sandbox": {"type": "workspaceWrite"}}})
        notify("mcpServer/startupStatus/updated", threadId=THREAD, name="studio", status="starting", error=None)
    elif method == "turn/start":
        run_turn(message)
    elif method == "thread/compact/start":
        run_command_turn(message, "compact")
    elif method == "review/start":
        run_command_turn(message, "review")
    elif method == "turn/interrupt":
        handle_side(message)
        queued.clear()
    elif method == "turn/steer":
        send({"id": message["id"], "error": {"code": -32600, "message": "no active turn to steer"}})
    elif "id" in message and "method" in message:
        send({"id": message["id"], "result": {}})
