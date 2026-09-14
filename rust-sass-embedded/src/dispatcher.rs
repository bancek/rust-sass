// Copyright 2023 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/embedded/isolate_dispatcher.dart
// go-source: go/embedded/isolate_dispatcher.go + go/embedded/reusable_isolate.go

#[cfg(feature = "async")]
use futures::StreamExt;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::io::{self, Read, Write};
use std::rc::Rc;
#[cfg(not(feature = "async"))]
use std::sync::mpsc::Receiver;
#[cfg(not(feature = "async"))]
use std::sync::mpsc::Sender;

use prost::Message;

use rust_sass::compile_context::new_compile_context;
use rust_sass::io::DefaultIo;
use rust_sass::Bump;

use crate::compilation::{handle_compile_request, CompilationContext, HostContext};
use crate::embedded_sass::outbound_message;
use crate::embedded_sass::{
    inbound_message, InboundMessage, OutboundMessage, ProtocolError, ProtocolErrorType,
};
use crate::error::{exit_code_for, params_error, parse_error, ERROR_ID, OUTBOUND_REQUEST_ID};
use crate::opaque_registry::OpaqueRegistry;
use crate::packet::{parse_packet, read_packet, serialize_packet, write_packet, SharedWriter};
use crate::{COMPILER_VERSION, PROTOCOL_VERSION};

/// Matches Go: `maxConcurrentCompilations` (Dart: 7 on 32-bit, 15 otherwise).
const MAX_CONCURRENT_COMPILATIONS: usize = 15;

/// Events from the reader thread.
enum ReaderEvent {
    Packet(Vec<u8>),
    Eof,
    Error(io::Error),
}

/// A compilation's inbound mailbox: the sender (held by the dispatcher) and the
/// receiver (moved into the compilation thread once spawned).
#[cfg(feature = "async")]
struct Mailbox {
    tx: futures::channel::mpsc::UnboundedSender<Vec<u8>>,
    rx: Option<futures::channel::mpsc::UnboundedReceiver<Vec<u8>>>,
}

/// Sync build: a plain std channel — the compilation thread blocks on recv()
/// while awaiting host responses (matching Go's synchronous per-isolate
/// goroutine receive).
#[cfg(not(feature = "async"))]
struct Mailbox {
    tx: Sender<Vec<u8>>,
    rx: Option<Receiver<Vec<u8>>>,
}

/// Dispatches messages between the host and per-compilation threads.
///
/// Matches Go: `IsolateDispatcher` (isolate_dispatcher.go) and the
/// `ReusableIsolate` mailbox/sendport mechanics.
///
/// The streams are owned (`Box<dyn Read + Send>` / `Arc<Mutex<Box<dyn Write +
/// Send>>>`) because the reader and compilation threads are spawned detached
/// (`std::thread::spawn`): on a category-2 fatal the process must exit
/// immediately (Go `ExitFn(os.Exit)`), without waiting for the reader thread
/// (which may be blocked on an open stdin) or other compilations. The shared
/// writer mirrors Go's `concurrentPacketWriter` (one packet per lock).
pub struct Dispatcher {
    reader: Box<dyn Read + Send>,
    writer: SharedWriter,
    stderr: SharedWriter,
}

impl Dispatcher {
    pub fn new(reader: Box<dyn Read + Send>, writer: SharedWriter, stderr: SharedWriter) -> Self {
        Dispatcher {
            reader,
            writer,
            stderr,
        }
    }

    /// Runs the dispatcher until stdin is closed or a fatal error occurs,
    /// returning the process exit code.
    pub fn listen(self) -> i32 {
        let Dispatcher {
            reader,
            writer,
            stderr,
        } = self;
        let (tx_in, rx_in) = crossbeam_channel::unbounded::<ReaderEvent>();
        let (tx_pool, rx_pool) = crossbeam_channel::bounded::<()>(MAX_CONCURRENT_COMPILATIONS);
        let (tx_done, rx_done) = crossbeam_channel::unbounded::<(u32, Option<Vec<u8>>, i32)>();
        let mut active: HashMap<u32, Mailbox> = HashMap::new();
        let mut pending: VecDeque<u32> = VecDeque::new();
        let mut exit_code = 0;

        let writer = writer.clone();
        let stderr = stderr.clone();

        // Reader thread: never does anything but read stdin and push to the
        // (unbounded) queue, so a full pool or a busy compilation can never
        // stall host input.
        let mut reader = reader;
        let reader_tx = tx_in.clone();
        std::thread::spawn(move || loop {
            match read_packet(&mut reader) {
                Ok(packet) => {
                    if reader_tx.send(ReaderEvent::Packet(packet)).is_err() {
                        break;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                    let _ = reader_tx.send(ReaderEvent::Eof);
                    break;
                }
                Err(e) => {
                    let _ = reader_tx.send(ReaderEvent::Error(e));
                    break;
                }
            }
        });
        drop(tx_in);

        loop {
            crossbeam_channel::select! {
                recv(rx_in) -> event => {
                    match event {
                        Ok(ReaderEvent::Eof) => break,
                        Ok(ReaderEvent::Error(e)) => {
                            let pe = ProtocolError {
                                r#type: ProtocolErrorType::Internal as i32,
                                id: ERROR_ID,
                                message: e.to_string(),
                            };
                            write_error_packet(&writer, ERROR_ID, &pe);
                            exit_code = 70;
                            break;
                        }
                        Ok(ReaderEvent::Packet(packet)) => {
                            match route_packet(&packet, &mut active, &mut pending, &tx_pool, &tx_done, &writer, &stderr) {
                                Ok(()) => {}
                                Err(code) => {
                                    exit_code = code;
                                    break;
                                }
                            }
                        }
                        Err(_) => break,
                    }
                }
                recv(rx_done) -> done => {
                    if let Ok((id, final_packet, code)) = done {
                        // Atomically: drop the mailbox sender (removing the id
                        // from active), free the pool slot, write the final
                        // packet, and record the exit code. The final packet is
                        // written and the id released in the same arm, so a host
                        // reusing the id can never race a stale active entry
                        // (dart-sass#2004).
                        active.remove(&id);
                        let _ = rx_pool.try_recv();
                        if let Some(packet) = final_packet {
                            write_packet_locked(&writer, &packet);
                        }
                        if code != 0 {
                            exit_code = code;
                        }
                        if let Some(pid) = pending.pop_front() {
                            let _ = tx_pool.try_send(());
                            spawn_compilation(pid, &mut active, &tx_done, &writer, &stderr);
                        }
                    }
                }
            }
        }

        // Shutdown: dropping the active senders makes any compilation thread
        // stuck awaiting a host response see its mailbox close (rx.next() ->
        // None) and unwind; the rx_done drain then blocks until every
        // compilation thread has finished and dropped its tx_done clone.
        active.clear();
        drop(tx_done);
        while let Ok((_, final_packet, code)) = rx_done.recv() {
            if let Some(packet) = final_packet {
                write_packet_locked(&writer, &packet);
            }
            if code != 0 {
                exit_code = code;
            }
        }
        exit_code
    }
}

/// Routes a single inbound packet.
fn route_packet(
    packet: &[u8],
    active: &mut HashMap<u32, Mailbox>,
    pending: &mut VecDeque<u32>,
    tx_pool: &crossbeam_channel::Sender<()>,
    tx_done: &crossbeam_channel::Sender<(u32, Option<Vec<u8>>, i32)>,
    writer: &SharedWriter,
    stderr: &SharedWriter,
) -> Result<(), i32> {
    let (id, buf) = parse_packet(packet).map_err(|pe| {
        write_error_packet(writer, ERROR_ID, &pe);
        exit_code_for(&pe)
    })?;

    if id == 0 {
        let inbound = InboundMessage::decode(buf).map_err(|e| {
            let pe = parse_error(&format!("Invalid protobuf message: {e}"));
            write_error_packet(writer, 0, &pe);
            76
        })?;
        return match inbound.message {
            Some(inbound_message::Message::VersionRequest(req)) => {
                let response = OutboundMessage {
                    message: Some(outbound_message::Message::VersionResponse(
                        outbound_message::VersionResponse {
                            id: req.id,
                            protocol_version: PROTOCOL_VERSION.into(),
                            compiler_version: COMPILER_VERSION.into(),
                            implementation_version: COMPILER_VERSION.into(),
                            implementation_name: "dart-sass".into(),
                        },
                    )),
                };
                let packet = serialize_packet(0, &response);
                write_packet_locked(writer, &packet);
                Ok(())
            }
            Some(_) => {
                let pe = params_error("Only VersionRequest may have wire ID 0.");
                write_error_packet(writer, 0, &pe);
                Err(76)
            }
            None => {
                let pe = parse_error("InboundMessage.message is not set.");
                write_error_packet(writer, 0, &pe);
                Err(76)
            }
        };
    }

    if let Some(mailbox) = active.get(&id) {
        #[cfg(feature = "async")]
        let _ = mailbox.tx.unbounded_send(packet.to_vec());
        #[cfg(not(feature = "async"))]
        let _ = mailbox.tx.send(packet.to_vec());
        return Ok(());
    }

    // New compilation: create the mailbox now (so late packets for a
    // pending compilation buffer), deliver the first (CompileRequest)
    // packet to it, then acquire a pool slot or queue.
    #[cfg(feature = "async")]
    let (tx, rx) = futures::channel::mpsc::unbounded::<Vec<u8>>();
    #[cfg(not(feature = "async"))]
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    #[cfg(feature = "async")]
    tx.unbounded_send(packet.to_vec()).unwrap();
    #[cfg(not(feature = "async"))]
    tx.send(packet.to_vec()).unwrap();
    active.insert(
        id,
        Mailbox {
            tx: tx.clone(),
            rx: Some(rx),
        },
    );
    if tx_pool.try_send(()).is_ok() {
        spawn_compilation(id, active, tx_done, writer, stderr);
    } else {
        pending.push_back(id);
    }
    Ok(())
}

/// Spawns a compilation thread. The closure captures only `Send` data; the
/// `!Send` arena/context/future are created inside the thread.
fn spawn_compilation(
    id: u32,
    active: &mut HashMap<u32, Mailbox>,
    tx_done: &crossbeam_channel::Sender<(u32, Option<Vec<u8>>, i32)>,
    writer: &SharedWriter,
    stderr: &SharedWriter,
) {
    let rx = active.get_mut(&id).unwrap().rx.take().unwrap();
    let tx_done = tx_done.clone();
    let writer = writer.clone();
    let stderr = stderr.clone();
    std::thread::spawn(move || {
        let arena = Bump::new();
        let host = Rc::new(HostContext {
            compilation_id: id,
            io: Rc::new(DefaultIo::new()),
            rx: Rc::new(RefCell::new(rx)),
            compile_context: new_compile_context(),
            protocol_version: PROTOCOL_VERSION,
            compiler_version: COMPILER_VERSION,
            outbound_request_id: OUTBOUND_REQUEST_ID,
            writer,
            stderr,
        });
        let ctx = Rc::new(CompilationContext {
            host,
            functions: Rc::new(RefCell::new(OpaqueRegistry::new())),
            mixins: Rc::new(RefCell::new(OpaqueRegistry::new())),
            arena: &arena,
        });
        #[cfg(feature = "async")]
        let result = futures::executor::block_on(run_compilation(ctx));
        #[cfg(not(feature = "async"))]
        let result = run_compilation(ctx);
        let _ = tx_done.send((id, result.0, result.1));
    });
}

/// Reads the compilation's first (CompileRequest) packet, runs the compile, and
/// returns the serialized final packet (CompileResponse or Error) plus an exit
/// code. LogEvents and host-callback requests are written directly to the
/// shared writer during the compile; only the final packet rides `tx_done` so
/// the release is atomic with its write.
#[rust_sass_macros::maybe_async]
async fn run_compilation<'compile, 'parse>(
    ctx: Rc<CompilationContext<'compile, 'parse>>,
) -> (Option<Vec<u8>>, i32)
where
    'compile: 'parse,
    'parse: 'compile,
{
    let compilation_id = ctx.host.compilation_id;
    let Some(packet) = next_packet(&ctx.host.rx).await else {
        return (None, 0); // mailbox closed (host gone); unwind silently
    };
    let (_, buf) = match parse_packet(&packet) {
        Ok(x) => x,
        Err(pe) => return (Some(error_packet(compilation_id, &pe)), exit_code_for(&pe)),
    };
    let inbound = match InboundMessage::decode(buf) {
        Ok(m) => m,
        Err(e) => {
            let pe = parse_error(&format!("Invalid protobuf message: {e}"));
            return (Some(error_packet(compilation_id, &pe)), exit_code_for(&pe));
        }
    };
    match inbound.message {
        Some(inbound_message::Message::CompileRequest(req)) => {
            match handle_compile_request(ctx, &req).await {
                Ok(resp) => {
                    let packet = serialize_packet(
                        compilation_id,
                        &OutboundMessage {
                            message: Some(outbound_message::Message::CompileResponse(resp)),
                        },
                    );
                    (Some(packet), 0)
                }
                Err(pe) => (Some(error_packet(compilation_id, &pe)), exit_code_for(&pe)),
            }
        }
        Some(inbound_message::Message::VersionRequest(_)) => {
            let pe = params_error("VersionRequest must have compilation ID 0.");
            (Some(error_packet(compilation_id, &pe)), exit_code_for(&pe))
        }
        Some(_) => {
            let pe = params_error(&format!(
                "This response doesn't match any outstanding requests in compilation {compilation_id}."
            ));
            (Some(error_packet(compilation_id, &pe)), exit_code_for(&pe))
        }
        None => {
            let pe = parse_error("InboundMessage.message is not set.");
            (Some(error_packet(compilation_id, &pe)), exit_code_for(&pe))
        }
    }
}

fn error_packet(compilation_id: u32, pe: &ProtocolError) -> Vec<u8> {
    serialize_packet(
        compilation_id,
        &OutboundMessage {
            message: Some(outbound_message::Message::Error(pe.clone())),
        },
    )
}

fn write_error_packet(writer: &SharedWriter, id: u32, pe: &ProtocolError) {
    let packet = error_packet(id, pe);
    write_packet_locked(writer, &packet);
}

/// Writes a full packet (adding the length varint) under the shared writer lock.
fn write_packet_locked(writer: &SharedWriter, packet: &[u8]) {
    if let Ok(mut w) = writer.lock() {
        let _ = write_packet(&mut *w, packet);
        let _ = w.flush();
    }
}

/// Receives the next inbound packet from a compilation's mailbox.
#[cfg(feature = "async")]
#[allow(clippy::items_after_test_module)]
// The `RefCell` borrow is held across `.await`. Sound: the embedded server
// drives `!Send` futures on a single-threaded executor and a compilation's
// mailbox has a single consumer, so nothing can borrow while suspended
// (same no-reentry rationale as rust-sass's critical-invariants.md).
#[allow(clippy::await_holding_refcell_ref)]
async fn next_packet(
    rx: &RefCell<futures::channel::mpsc::UnboundedReceiver<Vec<u8>>>,
) -> Option<Vec<u8>> {
    rx.borrow_mut().next().await
}

#[cfg(not(feature = "async"))]
#[allow(clippy::items_after_test_module)]
fn next_packet(rx: &RefCell<Receiver<Vec<u8>>>) -> Option<Vec<u8>> {
    rx.borrow_mut().recv().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedded_sass::inbound_message::compile_request;
    use crate::embedded_sass::{inbound_message as im, OutboundMessage};
    use std::io::Cursor;
    use std::sync::{Arc, Mutex};

    struct BufWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for BufWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn run_dispatcher(input: &[u8]) -> (Vec<u8>, i32) {
        let reader: Box<dyn Read + Send> = Box::new(Cursor::new(input.to_vec()));
        let buf = Arc::new(Mutex::new(Vec::new()));
        let writer: SharedWriter = Arc::new(Mutex::new(
            Box::new(BufWriter(buf.clone())) as Box<dyn Write + Send>
        ));
        let stderr: SharedWriter =
            Arc::new(Mutex::new(Box::new(Vec::new()) as Box<dyn Write + Send>));
        let d = Dispatcher::new(reader, writer, stderr);
        let code = d.listen();
        let out = buf.lock().unwrap().clone();
        (out, code)
    }

    fn version_request() -> Vec<u8> {
        let msg = InboundMessage {
            message: Some(im::Message::VersionRequest(im::VersionRequest { id: 0 })),
        };
        serialize_packet(0, &msg)
    }

    fn compile_request(source: &str) -> Vec<u8> {
        let msg = InboundMessage {
            message: Some(im::Message::CompileRequest(im::CompileRequest {
                input: Some(compile_request::Input::String(
                    compile_request::StringInput {
                        source: source.into(),
                        url: String::new(),
                        syntax: 0, // SCSS
                        importer: None,
                    },
                )),
                ..Default::default()
            })),
        };
        serialize_packet(1, &msg)
    }

    #[test]
    fn version_and_compile() {
        let mut input = Vec::new();
        write_packet(&mut input, &version_request()).unwrap();
        write_packet(&mut input, &compile_request("a { b: c }")).unwrap();
        let (out, code) = run_dispatcher(&input);
        assert_eq!(code, 0);

        let mut cursor = Cursor::new(out);
        let first = read_packet(&mut cursor).unwrap();
        let (id, buf) = parse_packet(&first).unwrap();
        assert_eq!(id, 0);
        let msg = OutboundMessage::decode(buf).unwrap();
        assert!(matches!(
            msg.message,
            Some(outbound_message::Message::VersionResponse(_))
        ));

        let second = read_packet(&mut cursor).unwrap();
        let (id, buf) = parse_packet(&second).unwrap();
        assert_eq!(id, 1);
        let msg = OutboundMessage::decode(buf).unwrap();
        assert!(matches!(
            msg.message,
            Some(outbound_message::Message::CompileResponse(_))
        ));
    }

    #[test]
    fn compile_error_is_normal_response() {
        let mut input = Vec::new();
        write_packet(&mut input, &version_request()).unwrap();
        write_packet(&mut input, &compile_request("a { color: }")).unwrap();
        let (out, code) = run_dispatcher(&input);
        assert_eq!(code, 0);

        let mut cursor = Cursor::new(out);
        read_packet(&mut cursor).unwrap(); // version response
        let packet = read_packet(&mut cursor).unwrap();
        let (_, buf) = parse_packet(&packet).unwrap();
        let msg = OutboundMessage::decode(buf).unwrap();
        assert!(matches!(
            msg.message,
            Some(outbound_message::Message::CompileResponse(resp))
                if resp.result.as_ref().is_some_and(|r| matches!(r, outbound_message::compile_response::Result::Failure(_)))
        ));
    }

    #[test]
    fn version_request_must_have_id_zero() {
        let msg = InboundMessage {
            message: Some(im::Message::CompileRequest(im::CompileRequest::default())),
        };
        let input = serialize_packet(0, &msg);
        let (out, code) = run_dispatcher(&input);
        assert_eq!(code, 76);
        let mut cursor = Cursor::new(out);
        let packet = read_packet(&mut cursor).unwrap();
        let (_, buf) = parse_packet(&packet).unwrap();
        let decoded = OutboundMessage::decode(buf).unwrap();
        assert!(matches!(
            decoded.message,
            Some(outbound_message::Message::Error(_))
        ));
    }
}
