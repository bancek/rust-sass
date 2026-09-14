// Interactive differential suite: drives both our embedded server (the real
// threaded `Dispatcher`, run in-process over an in-memory duplex) and the Dart
// embedded compiler (the golden reference) through the same host-callback
// exchanges, and asserts the two produce byte-identical outbound packet
// streams.
//
// Unlike `differential.rs` (which feeds a pre-baked stdin buffer), these cases
// exercise the interactive path: the compiler emits `CanonicalizeRequest` /
// `ImportRequest` / `FileImportRequest` / `FunctionCallRequest` packets and
// blocks awaiting our deterministic responses, plus LogEvents, source maps,
// and Path input.
//
// The Dart compiler path comes from the `DART_SASS_EMBEDDED` env var, or
// defaults to the sass-embedded npm package's bundled Dart compiler. The test
// skips if Dart isn't available.

mod common;

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use prost::Message;

use common::{dart_command, normalize};
use rust_sass_embedded::embedded_sass::inbound_message::compile_request;
use rust_sass_embedded::embedded_sass::outbound_message;
use rust_sass_embedded::embedded_sass::{
    inbound_message as im, inbound_message::function_call_response, value as proto_value,
    InboundMessage, OutboundMessage, Syntax, Value as ProtoValue,
};
use rust_sass_embedded::packet::{parse_packet, serialize_packet, write_packet, SharedWriter};
use rust_sass_embedded::Dispatcher;

/// How long a single request/response round-trip may take before the test
/// fails. Generous: Dart's first packet includes a cold start.
const ROUND_TRIP_TIMEOUT: Duration = Duration::from_secs(20);

/// A bidirectional byte channel. Each direction is `Arc<(Mutex<VecDeque<u8>>,
/// Condvar, AtomicBool)>`: the condvar wakes a blocked reader when the writer
/// appends, and the `AtomicBool` marks the channel closed (readers then see
/// EOF). Writes are atomic full packets under one lock, so a reader waiting for
/// a packet sees it whole.
#[derive(Clone)]
struct Duplex {
    inner: Arc<(Mutex<VecDeque<u8>>, Condvar, AtomicBool)>,
}

impl Duplex {
    fn new() -> Self {
        Duplex {
            inner: Arc::new((
                Mutex::new(VecDeque::new()),
                Condvar::new(),
                AtomicBool::new(false),
            )),
        }
    }

    /// Marks the channel closed: blocked reads return EOF and
    /// `recv_packet_with_timeout` returns `None`.
    fn close(&self) {
        let (_, cvar, closed) = &*self.inner;
        closed.store(true, Ordering::SeqCst);
        cvar.notify_all();
    }

    /// Reads one length-delimited packet, waiting at most `timeout` for the
    /// first byte. Returns `None` on timeout or close.
    fn recv_packet_with_timeout(&self, timeout: Duration) -> Option<Vec<u8>> {
        let deadline = Instant::now() + timeout;
        let mut len: u64 = 0;
        let mut shift = 0;
        loop {
            let b = self.read_byte(deadline)?;
            len |= u64::from(b & 0x7f) << shift;
            if b <= 0x7f {
                break;
            }
            shift += 7;
            if shift > 53 {
                return None;
            }
        }
        let mut packet = Vec::with_capacity(len as usize);
        for _ in 0..len {
            packet.push(self.read_byte(deadline)?);
        }
        Some(packet)
    }

    /// Reads a single byte, waiting on the condvar (with a deadline) for it to
    /// appear.
    fn read_byte(&self, deadline: Instant) -> Option<u8> {
        let (lock, cvar, closed) = &*self.inner;
        let mut q = lock.lock().unwrap();
        loop {
            if let Some(b) = q.pop_front() {
                return Some(b);
            }
            if closed.load(Ordering::SeqCst) {
                return None;
            }
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            let (guard, _) = cvar.wait_timeout(q, deadline - now).unwrap();
            q = guard;
        }
    }
}

impl Read for Duplex {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let (lock, cvar, closed) = &*self.inner;
        let mut q = lock.lock().unwrap();
        loop {
            if !q.is_empty() {
                let mut n = 0;
                while n < buf.len() {
                    match q.pop_front() {
                        Some(b) => {
                            buf[n] = b;
                            n += 1;
                        }
                        None => break,
                    }
                }
                return Ok(n);
            }
            if closed.load(Ordering::SeqCst) {
                return Ok(0);
            }
            q = cvar.wait(q).unwrap();
        }
    }
}

impl Write for Duplex {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let (lock, cvar, _) = &*self.inner;
        let mut q = lock.lock().unwrap();
        q.extend(buf.iter().copied());
        cvar.notify_all();
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A peer that speaks the embedded protocol.
trait Peer {
    /// Sends an inbound packet (already framed) to the compiler.
    fn send_packet(&mut self, packet: &[u8]);

    /// Receives the next outbound packet, waiting at most `timeout`.
    fn recv_packet(&mut self, timeout: Duration) -> Option<(u32, OutboundMessage)>;

    /// Closes the connection. For in-process peers this waits for the
    /// dispatcher to exit and asserts a clean exit code.
    fn close(&mut self);
}

/// Drives our real threaded `Dispatcher` in-process over two [`Duplex`]
/// channels (one per direction).
struct OurPeer {
    to_server: Duplex,
    from_server: Duplex,
    exit_rx: mpsc::Receiver<i32>,
    handle: Option<JoinHandle<()>>,
}

impl OurPeer {
    fn new() -> Self {
        let to_server = Duplex::new();
        let from_server = Duplex::new();
        let (exit_tx, exit_rx) = mpsc::channel();
        let reader: Box<dyn Read + Send> = Box::new(to_server.clone());
        let writer: SharedWriter = Arc::new(Mutex::new(
            Box::new(from_server.clone()) as Box<dyn Write + Send>
        ));
        let stderr: SharedWriter =
            Arc::new(Mutex::new(Box::new(Vec::new()) as Box<dyn Write + Send>));
        let dispatcher = Dispatcher::new(reader, writer, stderr);
        let handle = std::thread::spawn(move || {
            let code = dispatcher.listen();
            let _ = exit_tx.send(code);
        });
        OurPeer {
            to_server,
            from_server,
            exit_rx,
            handle: Some(handle),
        }
    }
}

impl Peer for OurPeer {
    fn send_packet(&mut self, packet: &[u8]) {
        let mut w = self.to_server.clone();
        write_packet(&mut w, packet).expect("writing to in-process server");
    }

    fn recv_packet(&mut self, timeout: Duration) -> Option<(u32, OutboundMessage)> {
        let packet = self.from_server.recv_packet_with_timeout(timeout)?;
        let (id, buf) = parse_packet(&packet).ok()?;
        let msg = OutboundMessage::decode(buf).ok()?;
        Some((id, msg))
    }

    fn close(&mut self) {
        self.to_server.close();
        let code = self
            .exit_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("in-process dispatcher did not exit after EOF");
        assert_eq!(code, 0, "in-process dispatcher returned exit code {code}");
        self.handle.take().unwrap().join().unwrap();
    }
}

/// Drives the Dart embedded compiler as a child process. A reader thread feeds
/// decoded packets into an mpsc channel so `recv_packet` gets a timeout.
struct DartPeer {
    child: Child,
    stdin: Option<ChildStdin>,
    rx: mpsc::Receiver<(u32, OutboundMessage)>,
}

impl DartPeer {
    fn new(command: &[String]) -> Self {
        let mut child: Child = Command::new(&command[0])
            .args(&command[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn Dart embedded compiler");
        let stdin = child.stdin.take();
        let mut stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(packet) = rust_sass_embedded::packet::read_packet(&mut stdout) {
                let Ok((id, buf)) = parse_packet(&packet) else {
                    continue;
                };
                let Ok(msg) = OutboundMessage::decode(buf) else {
                    continue;
                };
                if tx.send((id, msg)).is_err() {
                    break;
                }
            }
        });
        DartPeer { child, stdin, rx }
    }
}

impl Peer for DartPeer {
    fn send_packet(&mut self, packet: &[u8]) {
        let stdin = self.stdin.as_mut().unwrap();
        write_packet(stdin, packet).expect("writing to Dart");
        stdin.flush().expect("flushing Dart stdin");
    }

    fn recv_packet(&mut self, timeout: Duration) -> Option<(u32, OutboundMessage)> {
        match self.rx.recv_timeout(timeout) {
            Ok(pair) => Some(pair),
            Err(mpsc::RecvTimeoutError::Disconnected) => None,
            Err(mpsc::RecvTimeoutError::Timeout) => None,
        }
    }

    fn close(&mut self) {
        // Dropping stdin lets the compiler's dispatcher see EOF and exit.
        self.stdin.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A differential case: a `CompileRequest` plus a script that responds to the
/// compiler's host-callback requests with fixed, deterministic responses
/// (identical for both servers).
struct Case {
    name: &'static str,
    request: InboundMessage,
    respond: fn(&OutboundMessage) -> InboundMessage,
}

/// Drives `peer` through one case and returns the ordered, normalized outbound
/// packet stream (VersionResponse, then interleaved callback requests and
/// LogEvents, ending with CompileResponse).
fn drive(peer: &mut dyn Peer, case: &Case) -> Vec<(u32, Vec<u8>)> {
    let mut out = Vec::new();

    let version = InboundMessage {
        message: Some(im::Message::VersionRequest(im::VersionRequest { id: 0 })),
    };
    peer.send_packet(&serialize_packet(0, &version));
    let (id, msg) = peer
        .recv_packet(ROUND_TRIP_TIMEOUT)
        .expect("timeout awaiting VersionResponse");
    assert_eq!(id, 0);
    out.push(normalize(id, &msg));

    peer.send_packet(&serialize_packet(1, &case.request));
    loop {
        let (id, msg) = peer
            .recv_packet(ROUND_TRIP_TIMEOUT)
            .expect("timeout awaiting outbound packet");
        match msg.message {
            Some(outbound_message::Message::CompileResponse(_)) => {
                out.push(normalize(id, &msg));
                return out;
            }
            Some(outbound_message::Message::LogEvent(_)) => {
                // Fire-and-forget: record it in the stream, don't respond.
                out.push(normalize(id, &msg));
            }
            Some(
                outbound_message::Message::CanonicalizeRequest(_)
                | outbound_message::Message::ImportRequest(_)
                | outbound_message::Message::FileImportRequest(_)
                | outbound_message::Message::FunctionCallRequest(_),
            ) => {
                out.push(normalize(id, &msg));
                let resp = (case.respond)(&msg);
                peer.send_packet(&serialize_packet(1, &resp));
            }
            Some(outbound_message::Message::Error(pe)) => {
                panic!("protocol error: {pe:?}")
            }
            Some(outbound_message::Message::VersionResponse(_)) => {
                panic!("unexpected VersionResponse")
            }
            None => panic!("OutboundMessage.message is not set"),
        }
    }
}

// ---------- Request builders ----------

fn string_input(source: &str, url: &str) -> compile_request::Input {
    compile_request::Input::String(compile_request::StringInput {
        source: source.into(),
        url: url.into(),
        syntax: Syntax::Scss as i32,
        importer: None,
    })
}

fn compile_request(input: compile_request::Input) -> InboundMessage {
    InboundMessage {
        message: Some(im::Message::CompileRequest(im::CompileRequest {
            input: Some(input),
            ..Default::default()
        })),
    }
}

/// A `CompileRequest` with `global_functions`.
fn compile_with_functions(source: &str, functions: &[&str]) -> InboundMessage {
    let mut req = match compile_request(string_input(source, "")) {
        InboundMessage {
            message: Some(im::Message::CompileRequest(req)),
            ..
        } => req,
        _ => unreachable!(),
    };
    req.global_functions = functions.iter().map(|s| s.to_string()).collect();
    InboundMessage {
        message: Some(im::Message::CompileRequest(req)),
    }
}

/// A `CompileRequest` with a host importer (`importer_id: 1`).
fn compile_with_host_importer(source: &str) -> InboundMessage {
    let mut req = match compile_request(string_input(source, "https://example.com/main.scss")) {
        InboundMessage {
            message: Some(im::Message::CompileRequest(req)),
            ..
        } => req,
        _ => unreachable!(),
    };
    req.importers = vec![compile_request::Importer {
        non_canonical_scheme: vec![],
        importer: Some(compile_request::importer::Importer::ImporterId(1)),
    }];
    InboundMessage {
        message: Some(im::Message::CompileRequest(req)),
    }
}

/// A `CompileRequest` with `source_map` (optionally including sources).
fn compile_with_source_map(source: &str, include_sources: bool) -> InboundMessage {
    let mut req = match compile_request(string_input(source, "https://example.com/input.scss")) {
        InboundMessage {
            message: Some(im::Message::CompileRequest(req)),
            ..
        } => req,
        _ => unreachable!(),
    };
    req.source_map = true;
    req.source_map_include_sources = include_sources;
    InboundMessage {
        message: Some(im::Message::CompileRequest(req)),
    }
}

/// A `CompileRequest` with a `Path` input pointing at `path`.
fn compile_path(path: &str) -> InboundMessage {
    compile_request(compile_request::Input::Path(path.to_string()))
}

// ---------- Response scripts ----------

fn respond_host_function(msg: &OutboundMessage) -> InboundMessage {
    let Some(outbound_message::Message::FunctionCallRequest(req)) = &msg.message else {
        panic!("expected FunctionCallRequest, got {:?}", msg.message);
    };
    let success = ProtoValue {
        value: Some(proto_value::Value::Number(proto_value::Number {
            value: 2.0,
            numerators: vec!["px".into()],
            denominators: vec![],
        })),
    };
    InboundMessage {
        message: Some(im::Message::FunctionCallResponse(
            im::FunctionCallResponse {
                id: req.id,
                accessed_argument_lists: vec![],
                result: Some(function_call_response::Result::Success(success)),
            },
        )),
    }
}

fn respond_host_function_error(msg: &OutboundMessage) -> InboundMessage {
    let Some(outbound_message::Message::FunctionCallRequest(req)) = &msg.message else {
        panic!("expected FunctionCallRequest, got {:?}", msg.message);
    };
    InboundMessage {
        message: Some(im::Message::FunctionCallResponse(
            im::FunctionCallResponse {
                id: req.id,
                accessed_argument_lists: vec![],
                result: Some(function_call_response::Result::Error("boom".into())),
            },
        )),
    }
}

fn respond_host_importer(msg: &OutboundMessage) -> InboundMessage {
    match &msg.message {
        Some(outbound_message::Message::CanonicalizeRequest(req)) => InboundMessage {
            message: Some(im::Message::CanonicalizeResponse(
                im::CanonicalizeResponse {
                    id: req.id,
                    containing_url_unused: true,
                    result: Some(im::canonicalize_response::Result::Url(
                        "file:///foo.scss".into(),
                    )),
                },
            )),
        },
        Some(outbound_message::Message::ImportRequest(req)) => InboundMessage {
            message: Some(im::Message::ImportResponse(im::ImportResponse {
                id: req.id,
                result: Some(im::import_response::Result::Success(
                    im::import_response::ImportSuccess {
                        contents: "a {b: c}".into(),
                        syntax: Syntax::Scss as i32,
                        source_map_url: None,
                    },
                )),
            })),
        },
        other => panic!("expected CanonicalizeRequest or ImportRequest, got {other:?}"),
    }
}

/// No host callbacks expected; panics if the compiler asks for one.
fn respond_none(msg: &OutboundMessage) -> InboundMessage {
    panic!("unexpected host-callback request: {msg:?}");
}

// ---------- Cases ----------

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "host_function",
            request: compile_with_functions("a {b: add-one(1px)}", &["add-one($n)"]),
            respond: respond_host_function,
        },
        Case {
            name: "host_function_error",
            request: compile_with_functions("a {b: foo()}", &["foo()"]),
            respond: respond_host_function_error,
        },
        Case {
            name: "host_importer",
            request: compile_with_host_importer("@use \"foo\";"),
            respond: respond_host_importer,
        },
        Case {
            name: "warn",
            request: compile_request(string_input("@warn \"boom\";", "")),
            respond: respond_none,
        },
        Case {
            name: "debug",
            request: compile_request(string_input("@debug \"boom\";", "")),
            respond: respond_none,
        },
        Case {
            name: "warn_with_span",
            request: compile_request(string_input(
                "a { @warn \"boom\"; }",
                "https://example.com/input.scss",
            )),
            respond: respond_none,
        },
        Case {
            name: "debug_with_span",
            request: compile_request(string_input(
                "@debug \"boom\";",
                "https://example.com/input.scss",
            )),
            respond: respond_none,
        },
        Case {
            name: "warn_deprecation_debug",
            // Fires a `slash-div` DEPRECATION_WARNING LogEvent and then a DEBUG
            // LogEvent for the same source: exercises `deprecation_type`,
            // `[slash-div]` in formatted, the stack trace, and the deprecation
            // -> debug ordering in one case.
            request: compile_request(string_input("@debug (12px/4px);", "")),
            respond: respond_none,
        },
        Case {
            name: "source_map",
            request: compile_with_source_map("a {b: c}", false),
            respond: respond_none,
        },
        Case {
            name: "source_map_with_sources",
            request: compile_with_source_map("a {b: c}", true),
            respond: respond_none,
        },
        Case {
            name: "source_map_multi_line",
            request: compile_with_source_map("a {\n  b: c;\n  d: e;\n}", false),
            respond: respond_none,
        },
        Case {
            name: "path_input",
            request: compile_path(
                &std::env::temp_dir()
                    .join("rust-sass-embedded-interactive.scss")
                    .to_string_lossy(),
            ),
            respond: respond_none,
        },
    ]
}

#[test]
fn matches_dart_interactively() {
    let dart = match dart_command() {
        Some(cmd) => cmd,
        None => {
            eprintln!("Dart embedded compiler not found; skipping interactive differential");
            return;
        }
    };

    let mut failures = Vec::new();
    for case in cases() {
        if case.name == "path_input" {
            let path = std::env::temp_dir().join("rust-sass-embedded-interactive.scss");
            std::fs::write(&path, "a {b: 1px + 2px}").unwrap();
        }

        let mut ours = OurPeer::new();
        let our_out = drive(&mut ours, &case);
        ours.close();

        let mut dart_peer = DartPeer::new(&dart);
        let dart_out = drive(&mut dart_peer, &case);
        dart_peer.close();

        if our_out != dart_out {
            failures.push(format!(
                "{}:\n  ours: {:02x?}\n  dart: {:02x?}",
                case.name, our_out, dart_out
            ));
        }
    }
    if !failures.is_empty() {
        panic!(
            "{} interactive mismatches:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}
