//! HTTP for MCP settings with the system's `curl`, as [`crate::updates`] does, so Studio
//! needs no HTTP client: the streamable-HTTP probe and the registry search.
//!
//! A probe's request (URL, headers, body) goes to `curl --config -` on its input: header
//! values may be secrets and never reach a command line, where other programs could read
//! them.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use studio_harness::McpHttpServer;

use super::{PROTOCOL_VERSION, ProbeFailure, ProbeFailureKind as Kind, initialize_request};

/// The answer to one POST.
struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// `curl`, without a console window on Windows.
pub(super) fn curl() -> Command {
    #[allow(unused_mut)]
    let mut command = Command::new("curl");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// A `curl` config file line `name = "value"`, quoted the way curl reads it.
fn config_line(name: &str, value: &str) -> String {
    let quoted = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("{name} = \"{quoted}\"\n")
}

/// POSTs `body` to the server, its headers (and the session, once there is one) read by
/// curl from its input.
fn post(
    server: &McpHttpServer,
    session: Option<&str>,
    body: &Value,
    deadline: Instant,
) -> Result<Reply, ProbeFailure> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err(ProbeFailure::new(
            Kind::TimedOut,
            "It did not answer in time.",
        ));
    }
    let dir = tempfile::tempdir().map_err(|error| {
        ProbeFailure::new(
            Kind::Unreachable,
            format!("Cannot prepare the request: {error}"),
        )
    })?;
    let headers_file = dir.path().join("headers");
    let mut config = String::from("silent\nshow-error\n");
    config.push_str(&config_line("url", &server.url));
    config.push_str(&config_line("proto", "=http,https"));
    config.push_str(&config_line("request", "POST"));
    config.push_str(&config_line(
        "max-time",
        &format!("{:.1}", left.as_secs_f32()),
    ));
    config.push_str(&config_line("dump-header", &headers_file.to_string_lossy()));
    config.push_str(&config_line("header", "Content-Type: application/json"));
    config.push_str(&config_line(
        "header",
        "Accept: application/json, text/event-stream",
    ));
    if let Some(session) = session {
        config.push_str(&config_line(
            "header",
            &format!("Mcp-Session-Id: {session}"),
        ));
        config.push_str(&config_line(
            "header",
            &format!("MCP-Protocol-Version: {PROTOCOL_VERSION}"),
        ));
    }
    for (name, value) in &server.headers {
        config.push_str(&config_line("header", &format!("{name}: {value}")));
    }
    config.push_str(&config_line("data-binary", &body.to_string()));
    let mut child = curl()
        .args(["--config", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            ProbeFailure::new(
                Kind::Unreachable,
                format!("Cannot send the request: curl is unavailable ({error})"),
            )
        })?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(config.as_bytes());
    }
    let output = child.wait_with_output().map_err(|error| {
        ProbeFailure::new(Kind::Unreachable, format!("The request failed: {error}"))
    })?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(match output.status.code() {
            Some(28) => ProbeFailure::new(
                Kind::TimedOut,
                format!("It did not answer in time. {message}"),
            ),
            _ => ProbeFailure::new(Kind::Unreachable, message),
        });
    }
    let dumped = std::fs::read_to_string(&headers_file).unwrap_or_default();
    // Only the last block counts: an interim `100 Continue` comes first.
    let block = dumped
        .split("\r\n\r\n")
        .filter(|block| block.trim_start().starts_with("HTTP/"))
        .last()
        .unwrap_or("");
    let mut lines = block.lines();
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    let headers = lines
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.trim().to_owned(), value.trim().to_owned()))
        })
        .collect();
    Ok(Reply {
        status,
        headers,
        body: String::from_utf8_lossy(&output.stdout).into_owned(),
    })
}

/// The JSON-RPC result for request `id` in a reply: a JSON body, or the `data:` lines of
/// an event stream.
fn result_for(reply: &Reply, id: u64) -> Result<Value, ProbeFailure> {
    match reply.status {
        401 | 403 => {
            return Err(ProbeFailure::new(
                Kind::Refused,
                format!("It answered HTTP {}.", reply.status),
            ));
        }
        status if !(200..300).contains(&status) => {
            return Err(ProbeFailure::new(
                Kind::NotMcp,
                format!("It answered HTTP {status}."),
            ));
        }
        _ => {}
    }
    let streamed = reply
        .header("content-type")
        .is_some_and(|kind| kind.contains("text/event-stream"));
    let messages: Vec<Value> = if streamed {
        reply
            .body
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .filter_map(|data| serde_json::from_str(data.trim()).ok())
            .collect()
    } else {
        match serde_json::from_str::<Value>(&reply.body) {
            Ok(Value::Array(batch)) => batch,
            Ok(message) => vec![message],
            Err(_) => Vec::new(),
        }
    };
    let Some(message) = messages
        .into_iter()
        .find(|message| message["id"] == json!(id))
    else {
        let start: String = reply.body.trim().chars().take(160).collect();
        return Err(ProbeFailure::new(
            Kind::NotMcp,
            if start.is_empty() {
                "It answered with an empty body.".to_owned()
            } else {
                format!("It answered with something that is not MCP JSON: {start}")
            },
        ));
    };
    if let Some(error) = message.get("error") {
        return Err(ProbeFailure::new(
            Kind::Refused,
            format!(
                "It refused: {}",
                error["message"].as_str().unwrap_or("no reason given")
            ),
        ));
    }
    Ok(message["result"].clone())
}

pub(super) fn probe(
    server: &McpHttpServer,
    timeout: Duration,
) -> Result<Vec<String>, ProbeFailure> {
    let deadline = Instant::now() + timeout;
    let started = post(server, None, &initialize_request(), deadline)?;
    result_for(&started, 1)?;
    let session = started.header("mcp-session-id").map(str::to_owned);
    // A notification has no answer worth reading; a server that refuses it refuses the
    // next request too.
    post(
        server,
        session.as_deref(),
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        deadline,
    )?;
    let listed = post(
        server,
        session.as_deref(),
        &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
        deadline,
    )?;
    Ok(super::tool_names(&result_for(&listed, 2)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};

    #[test]
    fn config_values_are_quoted_for_curl() {
        assert_eq!(
            config_line("header", r#"X: a"b\c"#),
            "header = \"X: a\\\"b\\\\c\"\n"
        );
    }

    #[test]
    fn the_http_probe_sends_headers_keeps_the_session_and_reads_event_streams() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("server.py");
        std::fs::write(
            &script,
            r#"
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        if self.headers.get("Authorization") != "Bearer tok-1":
            self.send_response(401)
            self.end_headers()
            return
        if "id" not in body:
            self.send_response(202)
            self.end_headers()
            return
        if body["method"] == "initialize":
            reply = {"jsonrpc": "2.0", "id": body["id"], "result": {"protocolVersion": "2025-06-18", "capabilities": {}}}
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Mcp-Session-Id", "s-1")
            data = json.dumps(reply).encode()
        else:
            session = self.headers.get("Mcp-Session-Id")
            tools = [{"name": "search"}, {"name": "session-" + str(session)}]
            reply = {"jsonrpc": "2.0", "id": body["id"], "result": {"tools": tools}}
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            data = ("event: message\ndata: " + json.dumps(reply) + "\n\n").encode()
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

server = HTTPServer(("127.0.0.1", 0), Handler)
print(server.server_address[1], flush=True)
server.serve_forever()
"#,
        )
        .unwrap();
        let mut child = std::process::Command::new(studio_test_support::python())
            .arg(&script)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut port = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut port)
            .unwrap();
        let url = format!("http://127.0.0.1:{}/mcp", port.trim());
        let server = |token: &str| {
            McpHttpServer::new(
                "local",
                &url,
                vec![("Authorization".into(), format!("Bearer {token}"))],
            )
            .unwrap()
        };
        let tools = probe(&server("tok-1"), Duration::from_secs(20));
        let refused = probe(&server("wrong"), Duration::from_secs(20));
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(tools.unwrap(), vec!["search", "session-s-1"]);
        let refused = refused.unwrap_err();
        assert_eq!(refused.kind, Kind::Refused, "{refused}");
        assert!(refused.detail.contains("401"));
        assert!(refused.hint(true).contains("token"));
        let gone = probe(&server("tok-1"), Duration::from_secs(20)).unwrap_err();
        assert_eq!(gone.kind, Kind::Unreachable, "{gone}");
    }
}
