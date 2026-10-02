"""Protocol test peer; never used as a provider or qualification result."""
import json
import pathlib
import sys
import time

mode = sys.argv[1]
root = pathlib.Path(sys.argv[2])

def send(message):
    print(json.dumps({"jsonrpc": "2.0", **message}), flush=True)

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
