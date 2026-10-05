"""Scripted ACP v1 agent for workflow integration tests.

TEST FIXTURE ONLY: it proves transport/workflow behavior (handshake, streaming,
permissions, cancellation, process containment, crashes) and NEVER qualifies a
provider. It speaks newline-delimited JSON-RPC 2.0 on stdin/stdout.

Usage: python3 acp-agent.py ROOT
  ROOT   existing directory for evidence files. The process cwd is the draft;
         `session/new` params.cwd must equal it.

Environment:
  SCRIPTED_AGENT_INIT=exit   exit(3) on `initialize` (start failure)
  SCRIPTED_AGENT_INIT=auth   answer `session/new` with -32000 "Authentication required"

Turn script selection (`session/prompt`): when the first line of the prompt text
starts with `SCRIPT `, the rest of that line is the JSON script. Otherwise
ROOT/plan.json (`{"turns":[...],"default":{...}}`, re-read per prompt) supplies
`turns[i]`, with i = ROOT/prompt-counter before its atomic increment (>= len ->
`default`, else {"text":"done","write":{"edit.txt":"edited"}}).

Script fields, applied in this order (all optional):
  delay_ms, wait_file, text, thought, tool, tool_updates, tool_interleave, permission,
  write, write_b64, append, delete, mkdir, helper, escape, stderr,
  flood, flood_tools, wait_file_late, linger, malformed, crash, hang, stop_reason
See the per-field handling in `run_script` below.

Evidence files in ROOT: starts, session-params-<n>.json, session-params.json,
calls.jsonl, prompt-counter, prompts.jsonl, permission-<id>.json, cancel.json,
cancels.jsonl, helper.pid, escape.pid, set-mode.json, set-config.json.

Gates (files in ROOT the test creates): if `hold-session` exists, `session/new` is answered
only once `release-session` exists (the engine task can be changed in between).
`wait_file_late` waits for that file AFTER the script's writes/helpers, just before the
turn ends. `linger` makes the process ignore SIGTERM and stdin EOF (only SIGKILL ends
it), so shutdown takes its whole grace period. `flood_pace` `{"every":N,"sleep_ms":M}` sleeps M ms after every N flooded tool cards so a long
stream stays under the driver's 256-event queue (an unpaced burst of non-coalescing events
is DESIGNED to seal the producer). `tool_interleave` emits every tool call,
then every update, then every completion (instead of card by card).
"""
import base64
import fcntl
import json
import os
import pathlib
import queue
import shutil
import signal
import subprocess
import sys
import threading
import time

root = pathlib.Path(sys.argv[1])
PID = os.getpid()
SESSION_ID = f"sess-{PID}"
INIT_MODE = os.environ.get("SCRIPTED_AGENT_INIT", "")
HARD_CAP_SECONDS = 120
DEFAULT_SCRIPT = {"text": "done", "write": {"edit.txt": "edited"}}

OPTIONS = {
    "modes": {"currentModeId": "code", "availableModes": [{"id": "code", "name": "Code"}, {"id": "plan", "name": "Plan"}]},
    "configOptions": [{"id": "model", "name": "Model", "type": "select", "currentValue": "fast", "options": [{"value": "fast", "name": "Fast"}, {"value": "deep", "name": "Deep"}]}],
}
mode_id = "code"
model_value = "fast"
LINGER = False  # set by a `linger` script: ignore SIGTERM and stdin EOF

sys.stdout.reconfigure(encoding="utf-8")
sys.stdin.reconfigure(encoding="utf-8")

out_lock = threading.Lock()
inbox = queue.Queue()
active = None  # JSON-RPC id of the active prompt
cancelled = False
awaiting = {}  # permission request id -> script permission id
responses = {}  # permission request id -> client response message


class Cancelled(Exception):
    """The active turn was answered `cancelled`; abandon the script."""


# ---------------------------------------------------------------- plumbing


def send(message):
    line = json.dumps({"jsonrpc": "2.0", **message})
    with out_lock:
        sys.stdout.write(line + "\n")
        sys.stdout.flush()


def write_file(path, data, mode="w"):
    with open(path, mode) as handle:
        handle.write(data)
        handle.flush()
        os.fsync(handle.fileno())


def record(name, value):
    write_file(root / name, json.dumps(value))


def append_jsonl(name, value):
    write_file(root / name, json.dumps(value) + "\n", "a")


def bump(name):
    """Atomically increments the integer counter file; returns the value BEFORE the increment."""
    with open(root / ".counters.lock", "a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        try:
            path = root / name
            try:
                value = int(path.read_text().strip() or "0")
            except FileNotFoundError:
                value = 0
            write_file(path, str(value + 1))
            return value
        finally:
            fcntl.flock(lock, fcntl.LOCK_UN)


def update(body):
    send({"method": "session/update", "params": {"sessionId": SESSION_ID, "update": body}})


def chunk(text, kind="agent_message_chunk"):
    update({"sessionUpdate": kind, "content": {"type": "text", "text": text}})


def finish(reason="end_turn"):
    """Answers the active prompt exactly once."""
    global active
    if active is None:
        return
    rid, active = active, None
    send({"id": rid, "result": {"stopReason": reason}})


def die(code):
    sys.stdout.flush()
    sys.stderr.flush()
    os._exit(code)


def reader():
    for line in sys.stdin:
        inbox.put(line)
    inbox.put(None)


# ---------------------------------------------------------------- message handling


def handle(line):
    global active, cancelled, mode_id, model_value
    if line is None:
        while LINGER:
            time.sleep(1)
        die(0)
    if not line.strip():
        return
    try:
        message = json.loads(line)
    except ValueError:
        sys.stderr.write("scripted-agent: ignoring non-JSON input\n")
        sys.stderr.flush()
        return
    method = message.get("method")
    rid = message.get("id")
    params = message.get("params") or {}
    if method is None:
        if rid in awaiting:
            responses[rid] = message
            outcome = (message.get("result") or {}).get("outcome") or {"outcome": "error", "error": message.get("error")}
            record(f"permission-{awaiting.pop(rid)}.json", outcome)
        return
    append_jsonl("calls.jsonl", {"pid": PID, "method": method})
    if method == "initialize":
        if INIT_MODE == "exit":
            die(3)
        send({"id": rid, "result": {
            "protocolVersion": 1,
            "agentInfo": {"name": "scripted-agent", "version": "test"},
            "agentCapabilities": {"loadSession": False, "promptCapabilities": {"image": False}},
            "authMethods": [],
        }})
    elif method == "session/new":
        record("session-params.json", params)
        record(f"session-params-{start_index}.json", params)
        cwd = params.get("cwd")
        if not isinstance(cwd, str) or os.path.realpath(cwd) != os.path.realpath(os.getcwd()):
            send({"id": rid, "error": {"code": -32602, "message": f"cwd {cwd!r} != process cwd {os.getcwd()!r}"}})
        elif INIT_MODE == "auth":
            send({"id": rid, "error": {"code": -32000, "message": "Authentication required"}})
        else:
            hold = root / "hold-session"
            deadline = time.time() + HARD_CAP_SECONDS
            while hold.exists() and not (root / "release-session").exists() and time.time() < deadline:
                time.sleep(0.01)
            send({"id": rid, "result": {"sessionId": SESSION_ID, **OPTIONS}})
    elif method == "session/set_mode":
        record("set-mode.json", params)
        if params.get("modeId") in ("code", "plan"):
            mode_id = params["modeId"]
            send({"id": rid, "result": {}})
        else:
            send({"id": rid, "error": {"code": -32602, "message": "mode not supported"}})
    elif method == "session/set_config_option":
        record("set-config.json", params)
        if params.get("configId") == "model" and params.get("value") in ("fast", "deep"):
            model_value = params["value"]
            cfg = [dict(OPTIONS["configOptions"][0], currentValue=model_value)]
            send({"id": rid, "result": {"configOptions": cfg}})
        else:
            send({"id": rid, "error": {"code": -32602, "message": "rejected"}})
    elif method == "session/prompt":
        if active is not None:
            send({"id": rid, "error": {"code": -32602, "message": "a prompt is already active"}})
        else:
            run_prompt(message)
    elif method == "session/cancel":
        event = {"pid": PID, "session": params.get("sessionId"), "was_active": active is not None}
        record("cancel.json", event)
        append_jsonl("cancels.jsonl", event)
        if active is not None:
            cancelled = True
            finish("cancelled")
    elif rid is not None:
        send({"id": rid, "error": {"code": -32601, "message": f"method not found: {method}"}})


def pump(timeout=0.0):
    """Services queued client messages; blocks up to `timeout` for the first."""
    try:
        item = inbox.get(timeout=timeout) if timeout > 0 else inbox.get_nowait()
    except queue.Empty:
        return
    handle(item)
    while True:
        try:
            item = inbox.get_nowait()
        except queue.Empty:
            return
        handle(item)


def check():
    if cancelled:
        raise Cancelled


def wait_until(done):
    """Pumps until `done()` is true; honors cancel and the hard cap."""
    cap = time.time() + HARD_CAP_SECONDS
    while not done():
        check()
        if time.time() > cap:
            die(9)
        pump(0.005)
    check()


# ---------------------------------------------------------------- scripts


def prompt_text(params):
    return "\n".join(b.get("text", "") for b in params.get("prompt", []) if isinstance(b, dict) and b.get("type") == "text")


def pick_script(text):
    """Returns (script, script_index)."""
    first = text.split("\n", 1)[0]
    if first.startswith("SCRIPT "):
        return json.loads(first[len("SCRIPT "):]), None
    try:
        plan = json.loads((root / "plan.json").read_text())
    except FileNotFoundError:
        plan = {}
    index = bump("prompt-counter")
    turns = plan.get("turns", [])
    if index < len(turns):
        return turns[index], index
    return plan.get("default", DEFAULT_SCRIPT), index


def spawn_sleeper(escape, pid_file):
    kwargs = {"stdin": subprocess.DEVNULL, "stdout": subprocess.DEVNULL, "stderr": subprocess.DEVNULL}
    if escape:
        kwargs["start_new_session"] = True
    child = subprocess.Popen(["sleep", "60"], **kwargs)
    write_file(root / pid_file, f"{child.pid}\n", "a")


def tool_cards(spec):
    if spec is True:
        spec = 1
    if isinstance(spec, int):
        return [{"id": f"t{i}", "title": f"Tool {i}"} for i in range(spec)]
    return [{"id": c.get("id", f"t{i}"), "title": c.get("title", f"Tool {i}")} for i, c in enumerate(spec)]


def request_permission(spec):
    """Sends session/request_permission and blocks for the reply; returns the outcome."""
    pid = spec.get("id", "perm-1")
    request_id = pid
    awaiting[request_id] = pid
    send({"id": request_id, "method": "session/request_permission", "params": {
        "sessionId": SESSION_ID,
        "toolCall": {"toolCallId": spec.get("tool_id", f"call-{pid}"), "title": spec["title"]},
        "options": [
            {"optionId": "allow", "name": "Allow once", "kind": "allow_once"},
            {"optionId": "reject", "name": "Reject", "kind": "reject_once"},
        ],
    }})
    wait_until(lambda: request_id in responses)
    result = responses[request_id].get("result") or {}
    return result.get("outcome") or {"outcome": "error"}


def write_text(path, data, mode="w"):
    target = os.path.join(os.getcwd(), path)
    os.makedirs(os.path.dirname(target), exist_ok=True)
    with open(target, mode, encoding="utf-8", newline="") as handle:
        handle.write(data)
        handle.flush()
        os.fsync(handle.fileno())


def run_script(script):
    if "delay_ms" in script:
        end = time.time() + script["delay_ms"] / 1000
        wait_until(lambda: time.time() >= end)
    if "wait_file" in script:
        target = root / script["wait_file"]
        wait_until(target.exists)

    texts = script.get("text")
    if isinstance(texts, str):
        texts = [texts]
    for part in texts or []:
        chunk(part)
    if "thought" in script:
        chunk(script["thought"], "agent_thought_chunk")

    if script.get("tool"):
        extra = script.get("tool_updates", 1)
        cards = tool_cards(script["tool"])
        if script.get("tool_interleave"):
            for card in cards:
                update({"sessionUpdate": "tool_call", "toolCallId": card["id"], "title": card["title"], "kind": "other", "status": "pending"})
            for _ in range(extra):
                for card in cards:
                    update({"sessionUpdate": "tool_call_update", "toolCallId": card["id"], "status": "in_progress"})
            for card in cards:
                update({"sessionUpdate": "tool_call_update", "toolCallId": card["id"], "status": "completed"})
        else:
            for card in cards:
                update({"sessionUpdate": "tool_call", "toolCallId": card["id"], "title": card["title"], "kind": "other", "status": "pending"})
                for _ in range(extra):
                    update({"sessionUpdate": "tool_call_update", "toolCallId": card["id"], "status": "in_progress"})
                update({"sessionUpdate": "tool_call_update", "toolCallId": card["id"], "status": "completed"})

    allowed = True
    if "permission" in script:
        outcome = request_permission(script["permission"])
        if outcome.get("outcome") == "cancelled":
            finish("cancelled")
            raise Cancelled
        allowed = outcome.get("outcome") == "selected" and outcome.get("optionId") == "allow"

    if allowed:
        for path, data in script.get("write", {}).items():
            write_text(path, data)
        for path, data in script.get("write_b64", {}).items():
            target = os.path.join(os.getcwd(), path)
            os.makedirs(os.path.dirname(target), exist_ok=True)
            with open(target, "wb") as handle:
                handle.write(base64.b64decode(data))
                handle.flush()
                os.fsync(handle.fileno())
        for path, data in script.get("append", {}).items():
            write_text(path, data, "a")
        for path in script.get("delete", []):
            target = os.path.join(os.getcwd(), path)
            if os.path.isdir(target) and not os.path.islink(target):
                shutil.rmtree(target)
            elif os.path.lexists(target):
                os.remove(target)
        for path in script.get("mkdir", []):
            os.makedirs(os.path.join(os.getcwd(), path), exist_ok=True)

    if script.get("helper"):
        spawn_sleeper(False, "helper.pid")
    if script.get("escape"):
        spawn_sleeper(True, "escape.pid")
    if "stderr" in script:
        sys.stderr.write(script["stderr"])
        sys.stderr.flush()

    for i in range(script.get("flood", 0)):
        chunk(f"f{i} ")
        if i % 256 == 0:
            check()
            pump()
    pace = script.get("flood_pace") or {}
    pace_every, pace_sleep = int(pace.get("every", 0)), float(pace.get("sleep_ms", 0)) / 1000.0
    for i in range(script.get("flood_tools", 0)):
        update({"sessionUpdate": "tool_call", "toolCallId": f"ft{i}", "title": f"Flood tool {i}", "kind": "other", "status": "pending"})
        if i % 256 == 0:
            check()
            pump()
        if pace_every and i % pace_every == pace_every - 1:
            time.sleep(pace_sleep)

    if "wait_file_late" in script:
        wait_until((root / script["wait_file_late"]).exists)

    if script.get("linger"):
        global LINGER
        LINGER = True
        signal.signal(signal.SIGTERM, signal.SIG_IGN)

    if script.get("malformed"):
        # A protocol violation: the driver fails the session and kills the tree while
        # this process is still alive (combine with `linger`, or the process exits on the
        # stdin EOF the failure causes before the kill).
        with out_lock:
            sys.stdout.write("this is not json\n")
            sys.stdout.flush()
        wait_until(lambda: False)

    if "crash" in script:
        die(int(script["crash"]))
    if script.get("hang"):
        wait_until(lambda: False)

    finish(script.get("stop_reason", "end_turn"))


def run_prompt(message):
    global active, cancelled
    active = message["id"]
    cancelled = False
    params = message.get("params") or {}
    text = prompt_text(params)
    try:
        script, index = pick_script(text)
        if not isinstance(script, dict):
            raise ValueError("script must be a JSON object")
    except ValueError as error:
        rid, active = active, None
        send({"id": rid, "error": {"code": -32602, "message": f"bad script: {error}"}})
        return
    append_jsonl("prompts.jsonl", {"pid": PID, "session": params.get("sessionId"), "text": text[:8000], "cwd": os.getcwd(), "script_index": index})
    try:
        run_script(script)
    except Cancelled:
        finish("cancelled")  # no-op when the cancel path already answered
    finally:
        cancelled = False


# ---------------------------------------------------------------- main

start_index = bump("starts") + 1
threading.Thread(target=reader, daemon=True).start()
while True:
    handle(inbox.get())
