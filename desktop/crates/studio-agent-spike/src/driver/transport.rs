//! Bounded newline-delimited JSON transport over the adapter's pipes.
//!
//! The SDK parses and dispatches messages; this layer owns framing limits so an
//! oversized, truncated or malformed line fails the driver before the SDK sees it,
//! and so writes can never block a caller or the connection task.

use super::{
    events::{AgentFailure, FailureKind},
    runtime::Shared,
};
use futures::{SinkExt, channel::mpsc};
use parking_lot::{Condvar, Mutex};
use std::{
    collections::VecDeque,
    io::{self, BufRead, BufReader, Read, Write},
    pin::Pin,
    process::{ChildStderr, ChildStdin, ChildStdout},
    sync::{Arc, mpsc as std_mpsc},
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// Frames and bytes that may sit between this transport and the SDK's dispatch loop.
/// The SDK drains its line stream into an internal *unbounded* channel, so bounding our
/// own queue alone would not bound memory; credit is returned only when the dispatch
/// loop actually handles a message.
pub(super) const INGRESS_MAX_FRAMES: usize = 32;
pub(super) const INGRESS_MAX_BYTES: usize = 8 * 1024 * 1024;
/// Without any dispatch progress for this long the driver fails visibly.
pub(super) const INGRESS_STALL: Duration = Duration::from_secs(10);

#[derive(Debug, PartialEq, Eq)]
pub(super) enum IngressError {
    Stalled,
    Closed,
}

struct IngressState {
    frames: VecDeque<usize>,
    bytes: usize,
    closed: bool,
}

/// Credit gate between the stdout reader and the SDK dispatch loop.
pub(super) struct Ingress {
    state: Mutex<IngressState>,
    changed: Condvar,
    max_frames: usize,
    max_bytes: usize,
}

impl Ingress {
    pub(super) fn new(max_frames: usize, max_bytes: usize) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(IngressState {
                frames: VecDeque::new(),
                bytes: 0,
                closed: false,
            }),
            changed: Condvar::new(),
            max_frames,
            max_bytes,
        })
    }

    /// Reserves credit for one frame, waiting for dispatch progress when the window is
    /// full. At least one frame is always admitted so a single large frame cannot deadlock.
    pub(super) fn acquire(&self, len: usize, stall: Duration) -> Result<(), IngressError> {
        let mut state = self.state.lock();
        let mut deadline = Instant::now() + stall;
        loop {
            if state.closed {
                return Err(IngressError::Closed);
            }
            let full = state.frames.len() >= self.max_frames
                || (!state.frames.is_empty() && state.bytes + len > self.max_bytes);
            if !full {
                state.frames.push_back(len);
                state.bytes += len;
                return Ok(());
            }
            let before = state.frames.len();
            if self.changed.wait_until(&mut state, deadline).timed_out() {
                return Err(IngressError::Stalled);
            }
            if state.frames.len() < before {
                // Progress was made; restart the stall window.
                deadline = Instant::now() + stall;
            }
        }
    }

    /// Called once per message the SDK dispatches, in arrival order.
    pub(super) fn release_one(&self) {
        let mut state = self.state.lock();
        if let Some(len) = state.frames.pop_front() {
            state.bytes -= len;
        }
        self.changed.notify_all();
    }

    pub(super) fn close(&self) {
        self.state.lock().closed = true;
        self.changed.notify_all();
    }

    pub(super) fn in_flight(&self) -> usize {
        self.state.lock().frames.len()
    }
}

pub(super) type LineSink = Pin<Box<dyn futures::Sink<String, Error = io::Error> + Send>>;
pub(super) type LineStream = Pin<Box<dyn futures::Stream<Item = io::Result<String>> + Send>>;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum FrameError {
    Oversized,
    Truncated,
    Malformed(String),
    /// A JSON array: batches would dispatch several messages per frame.
    Batch,
    Io(String),
}

/// Reads one non-empty line of at most `max` bytes (newline included).
pub(super) fn read_frame(
    reader: &mut impl BufRead,
    max: usize,
) -> Result<Option<String>, FrameError> {
    loop {
        let mut bytes = Vec::new();
        let n = reader
            .take((max + 1) as u64)
            .read_until(b'\n', &mut bytes)
            .map_err(|e| FrameError::Io(e.to_string()))?;
        if n == 0 {
            return Ok(None);
        }
        if n > max {
            return Err(FrameError::Oversized);
        }
        if bytes.last() != Some(&b'\n') {
            return Err(FrameError::Truncated);
        }
        while matches!(bytes.last(), Some(b'\n' | b'\r')) {
            bytes.pop();
        }
        if bytes.is_empty() {
            continue;
        }
        let line = String::from_utf8(bytes)
            .map_err(|_| FrameError::Malformed("message is not valid UTF-8".into()))?;
        serde_json::from_str::<serde::de::IgnoredAny>(&line)
            .map_err(|e| FrameError::Malformed(format!("message is not valid JSON: {e}")))?;
        if line.trim_start().starts_with('[') {
            return Err(FrameError::Batch);
        }
        return Ok(Some(line));
    }
}

pub(super) fn spawn_reader(
    shared: Arc<Shared>,
    stdout: ChildStdout,
    mut tx: mpsc::Sender<io::Result<String>>,
    ingress: Arc<Ingress>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("acp-stdout".into())
        .spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_frame(&mut reader, shared.limits.max_message_bytes) {
                    Ok(Some(line)) => {
                        match ingress.acquire(line.len(), INGRESS_STALL) {
                            Ok(()) => {}
                            Err(IngressError::Closed) => break,
                            Err(IngressError::Stalled) => {
                                shared.fail(AgentFailure::new(
                                    FailureKind::IngressStalled,
                                    shared.phase(),
                                    "the connection stopped dispatching adapter messages",
                                ));
                                break;
                            }
                        }
                        if futures::executor::block_on(tx.send(Ok(line))).is_err() {
                            break;
                        }
                    }
                    // Clean EOF: dropping the sender closes the SDK's incoming stream.
                    Ok(None) => break,
                    Err(error) => {
                        let (kind, message) = match error {
                            FrameError::Oversized => (
                                FailureKind::OversizedMessage,
                                format!(
                                    "adapter message exceeds {} bytes",
                                    shared.limits.max_message_bytes
                                ),
                            ),
                            FrameError::Truncated => (
                                FailureKind::TruncatedMessage,
                                "adapter closed stdout in the middle of a message".to_owned(),
                            ),
                            FrameError::Malformed(m) => (FailureKind::MalformedMessage, m),
                            FrameError::Batch => (
                                FailureKind::ProtocolViolation,
                                "batched JSON-RPC messages are not supported".to_owned(),
                            ),
                            FrameError::Io(m) => (FailureKind::ProcessExited, m),
                        };
                        shared.fail(AgentFailure::new(kind, shared.phase(), message.clone()));
                        let _ =
                            futures::executor::block_on(tx.send(Err(io::Error::other(message))));
                        break;
                    }
                }
            }
        })
        .expect("spawn acp stdout reader")
}

pub(super) fn spawn_stderr(shared: Arc<Shared>, mut stderr: ChildStderr) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("acp-stderr".into())
        .spawn(move || {
            let mut chunk = [0u8; 8192];
            loop {
                match stderr.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => shared.stderr_push(&chunk[..n]),
                }
            }
            shared.stderr_finish();
        })
        .expect("spawn acp stderr reader")
}

/// Writer thread plus the SDK-facing sink. A full queue means the adapter is not
/// reading: that fails the driver instead of blocking the connection task.
pub(super) fn spawn_writer(
    shared: Arc<Shared>,
    mut stdin: ChildStdin,
) -> (LineSink, JoinHandle<()>) {
    let (tx, rx) = std_mpsc::sync_channel::<Vec<u8>>(24);
    let handle = std::thread::Builder::new()
        .name("acp-stdin".into())
        .spawn(move || {
            while let Ok(bytes) = rx.recv() {
                if stdin.write_all(&bytes).and_then(|_| stdin.flush()).is_err() {
                    break;
                }
            }
        })
        .expect("spawn acp stdin writer");
    let sink = futures::sink::unfold((tx, shared), async |(tx, shared), line: String| {
        let mut bytes = line.into_bytes();
        if bytes.len() >= shared.limits.max_message_bytes {
            let failure = AgentFailure::new(
                FailureKind::OversizedOutbound,
                shared.phase(),
                "outbound ACP message exceeds the wire limit",
            );
            shared.fail(failure);
            return Err(io::Error::other("outbound ACP message exceeds limit"));
        }
        bytes.push(b'\n');
        match tx.try_send(bytes) {
            Ok(()) => Ok((tx, shared)),
            Err(std_mpsc::TrySendError::Full(_)) => {
                shared.fail(AgentFailure::new(
                    FailureKind::WriteBlocked,
                    shared.phase(),
                    "adapter is not reading stdin; write queue is full",
                ));
                Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "adapter stdin is blocked",
                ))
            }
            Err(std_mpsc::TrySendError::Disconnected(_)) => Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "adapter stdin closed",
            )),
        }
    });
    (Box::pin(sink), handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn frame(bytes: &[u8], max: usize) -> Result<Option<String>, FrameError> {
        read_frame(&mut BufReader::new(Cursor::new(bytes.to_vec())), max)
    }

    #[test]
    fn accepts_a_complete_json_line_and_skips_blank_lines() {
        assert_eq!(
            frame(b"\n\r\n{\"jsonrpc\":\"2.0\"}\r\n", 100).unwrap(),
            Some("{\"jsonrpc\":\"2.0\"}".into())
        );
        assert_eq!(frame(b"", 100).unwrap(), None);
    }

    #[test]
    fn rejects_oversized_truncated_and_malformed_lines() {
        let mut big = vec![b'x'; 101];
        big.push(b'\n');
        assert_eq!(frame(&big, 100), Err(FrameError::Oversized));
        assert_eq!(frame(b"{\"a\":1}", 100), Err(FrameError::Truncated));
        assert!(matches!(
            frame(b"not json\n", 100),
            Err(FrameError::Malformed(_))
        ));
        assert!(matches!(
            frame(&[0xff, 0xfe, b'\n'], 100),
            Err(FrameError::Malformed(_))
        ));
    }

    #[test]
    fn batches_are_rejected_before_the_sdk_sees_them() {
        assert_eq!(
            frame(b"[{\"jsonrpc\":\"2.0\"}]\n", 100),
            Err(FrameError::Batch)
        );
    }

    #[test]
    fn ingress_credit_is_returned_only_on_dispatch_and_stalls_visibly() {
        let ingress = Ingress::new(2, 1000);
        ingress.acquire(10, Duration::from_millis(50)).unwrap();
        ingress.acquire(10, Duration::from_millis(50)).unwrap();
        assert_eq!(ingress.in_flight(), 2);
        // Window full and nothing dispatched: the reader stalls instead of queueing.
        assert_eq!(
            ingress.acquire(10, Duration::from_millis(60)),
            Err(IngressError::Stalled)
        );
        // A dispatch from another thread frees exactly one slot.
        let releaser = ingress.clone();
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            releaser.release_one();
        });
        ingress.acquire(10, Duration::from_secs(5)).unwrap();
        handle.join().unwrap();
        assert_eq!(ingress.in_flight(), 2);
        ingress.close();
        assert_eq!(
            ingress.acquire(1, Duration::from_millis(10)),
            Err(IngressError::Closed)
        );
    }

    #[test]
    fn ingress_bounds_bytes_but_always_admits_one_large_frame() {
        let ingress = Ingress::new(100, 100);
        ingress.acquire(1000, Duration::from_millis(20)).unwrap();
        assert_eq!(
            ingress.acquire(1, Duration::from_millis(30)),
            Err(IngressError::Stalled)
        );
        ingress.release_one();
        ingress.acquire(60, Duration::from_millis(20)).unwrap();
        ingress.acquire(30, Duration::from_millis(20)).unwrap();
        assert_eq!(
            ingress.acquire(30, Duration::from_millis(30)),
            Err(IngressError::Stalled)
        );
    }

    #[test]
    fn a_line_of_exactly_the_limit_is_accepted() {
        let mut line = b"\"".to_vec();
        line.extend(std::iter::repeat_n(b'a', 97));
        line.extend_from_slice(b"\"\n");
        assert_eq!(line.len(), 100);
        assert!(frame(&line, 100).unwrap().is_some());
    }
}
