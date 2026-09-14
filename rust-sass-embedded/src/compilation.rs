// Copyright 2019 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/embedded/compilation_dispatcher.dart (context + requests)
// go-source: go/embedded/compilation_dispatcher.go

use crate::syntax_from_proto;
#[cfg(test)]
use crate::COMPILER_VERSION;
use std::cell::RefCell;
#[cfg(test)]
use std::io::Write;
use std::rc::Rc;
#[cfg(not(feature = "async"))]
use std::sync::mpsc::Receiver;
#[cfg(test)]
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;

#[cfg(feature = "async")]
use futures::StreamExt;
use prost::Message;

use rust_sass::common::exception::SassError;
use rust_sass::compile::{compile, compile_string, CompileOptions, OutputStyle};
use rust_sass::compile_context::CompileContext;
use rust_sass::deprecation::{self, Deprecation};
use rust_sass::eval::importer::filesystem::FilesystemImporter;
use rust_sass::eval::importer::node_package::NodePackageImporter;
use rust_sass::eval::importer::{Importer, ImporterKind};
use rust_sass::io::Io;
use rust_sass::logger::{Logger, QuietLogger};
use rust_sass::url::SassUrl;
use rust_sass::value::{SassFunction, SassMixin};
use rust_sass::Bump;

use crate::embedded_sass::inbound_message::compile_request;
use crate::embedded_sass::outbound_message;
use crate::embedded_sass::outbound_message::compile_response;
use crate::embedded_sass::{inbound_message, InboundMessage, OutboundMessage, ProtocolError};
use crate::error::{build_failure, highlight_options, mandatory_error, params_error, parse_error};
use crate::function::host_callable;
use crate::importer_file::FileImporter;
use crate::importer_host::HostImporter;
use crate::logger::EmbeddedLogger;
use crate::opaque_registry::OpaqueRegistry;
use crate::packet::{parse_packet, write_message_shared, SharedWriter};

/// The `'static` protocol-facing context shared by the logger, importers, and
/// host-function callbacks (all of which must be `dyn Logger`/`dyn UserImporter`
/// = `'static` trait objects, so they can only hold `'static` data — the same
/// constraint `EvalConfig.logger: Rc<dyn Logger>` enforces in rust-sass).
///
/// Matches Go: fields on `CompilationDispatcher`.
pub struct HostContext {
    pub compilation_id: u32,
    pub io: Rc<dyn Io>,
    /// Mailbox for inbound host responses; host callbacks read from it.
    #[cfg(feature = "async")]
    pub rx: Rc<RefCell<futures::channel::mpsc::UnboundedReceiver<Vec<u8>>>>,
    #[cfg(not(feature = "async"))]
    pub rx: Rc<RefCell<Receiver<Vec<u8>>>>,
    pub compile_context: CompileContext,
    pub protocol_version: &'static str,
    pub compiler_version: &'static str,
    /// The request ID used for every outbound (compiler → host) request.
    pub outbound_request_id: u32,
    /// Shared outbound packet writer (LogEvents, host-callback requests).
    pub writer: SharedWriter,
    /// Shared stderr for error diagnostics.
    pub stderr: SharedWriter,
}

impl HostContext {
    /// Sends an outbound request to the host and awaits its matching response
    /// from the mailbox.
    ///
    /// Matches Go: `sendFunctionCallRequest`/`canonicalize`/`importUrl`/
    /// `redirect` (the generic send-and-await), and the id/type validation in
    /// `CompilationDispatcher`'s message handling.
    #[cfg(feature = "async")]
    // The mailbox `RefCell` borrow is held across `.await`. Sound: `!Send`
    // futures on a single-threaded executor with a single consumer per
    // compilation mailbox (see `next_packet` in dispatcher.rs).
    #[allow(clippy::await_holding_refcell_ref)]
    pub async fn send_and_wait(
        &self,
        msg: &OutboundMessage,
    ) -> Result<inbound_message::Message, HostError> {
        write_message_shared(&self.writer, self.compilation_id, msg)
            .map_err(|_| HostError::Protocol(params_error("Failed to write to host")))?;
        let packet = self
            .rx
            .borrow_mut()
            .next()
            .await
            .ok_or(HostError::MailboxClosed)?;
        let (_, buf) = parse_packet(&packet).map_err(HostError::Protocol)?;
        let inbound = InboundMessage::decode(buf)
            .map_err(|e| HostError::Protocol(parse_error(&e.to_string())))?;
        match inbound.message {
            Some(inbound_message::Message::CompileRequest(_)) => {
                Err(HostError::Protocol(params_error(&format!(
                    "A CompileRequest with compilation ID {} is already active.",
                    self.compilation_id
                ))))
            }
            Some(inbound_message::Message::VersionRequest(_)) => Err(HostError::Protocol(
                params_error("VersionRequest must have compilation ID 0."),
            )),
            Some(m) => Ok(m),
            None => Err(HostError::Protocol(parse_error(
                "InboundMessage.message is not set.",
            ))),
        }
    }
    #[cfg(not(feature = "async"))]
    pub fn send_and_wait(
        &self,
        msg: &OutboundMessage,
    ) -> Result<inbound_message::Message, HostError> {
        write_message_shared(&self.writer, self.compilation_id, msg)
            .map_err(|_| HostError::Protocol(params_error("Failed to write to host")))?;
        let packet = self
            .rx
            .borrow_mut()
            .recv()
            .map_err(|_| HostError::MailboxClosed)?;
        let (_, buf) = parse_packet(&packet).map_err(HostError::Protocol)?;
        let inbound = InboundMessage::decode(buf)
            .map_err(|e| HostError::Protocol(parse_error(&e.to_string())))?;
        match inbound.message {
            Some(inbound_message::Message::CompileRequest(_)) => {
                Err(HostError::Protocol(params_error(&format!(
                    "A CompileRequest with compilation ID {} is already active.",
                    self.compilation_id
                ))))
            }
            Some(inbound_message::Message::VersionRequest(_)) => Err(HostError::Protocol(
                params_error("VersionRequest must have compilation ID 0."),
            )),
            Some(m) => Ok(m),
            None => Err(HostError::Protocol(parse_error(
                "InboundMessage.message is not set.",
            ))),
        }
    }
}

/// The `'compile`/`'parse`-carrying per-compilation context: the value
/// registries and the arena (used by the protofier and the compile path), plus
/// a handle to the `'static` [`HostContext`]. Created inside the compilation
/// thread. Follows the eval convention: `'compile` (arena) outlives `'parse`
/// (arena-allocated data); callables additionally require `'parse: 'compile`.
pub struct CompilationContext<'compile, 'parse> {
    pub host: Rc<HostContext>,
    /// Compiler-defined values sent to the host (as `CompilerFunction`s).
    pub functions: Rc<RefCell<OpaqueRegistry<SassFunction<'parse>>>>,
    /// Compiler-defined values sent to the host (as `CompilerMixin`s).
    pub mixins: Rc<RefCell<OpaqueRegistry<SassMixin<'parse>>>>,
    /// The compilation's arena (source text + host signature parsing).
    pub arena: &'compile Bump,
}

/// An error produced while awaiting a host response.
pub enum HostError {
    /// The host violated the protocol (bad/missing response, etc.).
    Protocol(ProtocolError),
    /// The host closed the connection mid-request; the compilation should
    /// unwind silently (no response is written).
    MailboxClosed,
}

impl From<HostError> for Box<SassError> {
    fn from(e: HostError) -> Self {
        Box::new(e.into())
    }
}

impl From<HostError> for SassError {
    fn from(e: HostError) -> Self {
        // Note: Dart/Go terminate the process (exit 76) on a protocol error
        // during a host callback; we instead surface it as a compile failure.
        match e {
            HostError::Protocol(pe) => SassError::Script {
                message: pe.message,
                argument_name: None,
            },
            HostError::MailboxClosed => SassError::Script {
                message: "The connection to the host was closed.".into(),
                argument_name: None,
            },
        }
    }
}

/// An error from `decode_importer`: either a protocol error (category-2, host
/// violated the protocol) or a `SassError` (compile failure).
enum DecodeImporterError {
    Protocol(ProtocolError),
    Sass(Box<SassError>),
}

impl From<DecodeImporterError> for ProtocolError {
    fn from(e: DecodeImporterError) -> Self {
        match e {
            DecodeImporterError::Protocol(pe) => pe,
            DecodeImporterError::Sass(err) => params_error(&err.full_message()),
        }
    }
}

/// Runs a compilation and returns the `CompileResponse` (or a `ProtocolError`
/// for category-2 fatal errors).
///
/// Matches Go: `CompilationDispatcher.compile`.
#[rust_sass_macros::maybe_async]
pub async fn handle_compile_request<'compile, 'parse>(
    ctx: Rc<CompilationContext<'compile, 'parse>>,
    req: &inbound_message::CompileRequest,
) -> Result<outbound_message::CompileResponse, ProtocolError>
where
    'compile: 'parse,
    'parse: 'compile,
{
    ctx.functions.borrow_mut().clear();
    ctx.mixins.borrow_mut().clear();

    let style = if req.style == OutputStyle::Compressed as i32 {
        OutputStyle::Compressed
    } else {
        OutputStyle::Expanded
    };

    let logger: Rc<dyn Logger> = if req.silent {
        Rc::new(QuietLogger)
    } else {
        Rc::new(EmbeddedLogger::new(
            ctx.host.clone(),
            req.alert_color,
            req.alert_ascii,
        ))
    };

    let fatal_deprecations =
        parse_deprecations_or_warn(&req.fatal_deprecation, true, logger.clone());
    let silence_deprecations =
        parse_deprecations_or_warn(&req.silence_deprecation, false, logger.clone());
    let future_deprecations =
        parse_deprecations_or_warn(&req.future_deprecation, false, logger.clone());

    let highlight = highlight_options(req.alert_color, req.alert_ascii);

    let mut importers = Vec::new();
    for imp in &req.importers {
        let decoded = match decode_importer(ctx.arena, ctx.host.clone(), imp) {
            Ok(d) => d,
            Err(DecodeImporterError::Protocol(pe)) => return Err(pe),
            // Dart: an invalid scheme is a `SassException` from the importer
            // constructor, caught by `_compile`'s `on SassException` and sent as
            // a `CompileFailure` (not a protocol error).
            Err(DecodeImporterError::Sass(err)) => {
                return Ok(build_failure_response(&ctx.host, &err, None, &highlight))
            }
        };
        match decoded {
            Some(i) => importers.push(i),
            None => return Err(mandatory_error("Importer.importer")),
        }
    }

    let mut global_functions = Vec::new();
    for sig in &req.global_functions {
        match host_callable(ctx.clone(), sig, None, ctx.arena) {
            Ok(c) => global_functions.push(c),
            Err(e) => return Ok(build_failure_response(&ctx.host, &e, None, &highlight)),
        }
    }

    let (result, entrypoint) = match &req.input {
        Some(compile_request::Input::String(input)) => {
            let syntax = match syntax_from_proto(input.syntax) {
                Ok(s) => s,
                Err(e) => return Ok(build_failure_response(&ctx.host, &e, None, &highlight)),
            };
            let entrypoint = if input.url.is_empty() {
                None
            } else {
                SassUrl::parse(&input.url).ok()
            };
            let mut opts = base_options(
                ctx.arena,
                req,
                importers,
                global_functions,
                logger,
                style,
                fatal_deprecations,
                silence_deprecations,
                future_deprecations,
            );
            opts.syntax = syntax;
            opts.url = entrypoint.clone();
            let entrypoint_importer = match input.importer.as_ref() {
                Some(imp) => decode_importer(ctx.arena, ctx.host.clone(), imp)?,
                None => None,
            };
            opts.importer = match entrypoint_importer {
                Some(i) => i,
                None => Importer::new(ctx.arena, ImporterKind::NoOp),
            };
            (
                compile_string(&input.source, ctx.host.io.clone(), opts, ctx.arena).await,
                entrypoint,
            )
        }
        Some(compile_request::Input::Path(path)) => {
            if path.is_empty() {
                return Err(mandatory_error("CompileRequest.Input.path"));
            }
            let opts = base_options(
                ctx.arena,
                req,
                importers,
                global_functions,
                logger,
                style,
                fatal_deprecations,
                silence_deprecations,
                future_deprecations,
            );
            let entrypoint = SassUrl::parse(path).ok();
            (
                compile(path, ctx.host.io.clone(), opts, ctx.arena).await,
                entrypoint,
            )
        }
        None => return Err(mandatory_error("CompileRequest.input")),
    };

    match result {
        Ok(result) => {
            let mut success = compile_response::CompileSuccess {
                css: result.css().to_string(),
                source_map: String::new(),
            };
            if let Some(sm) = result.source_map() {
                let mut sm = sm.clone();
                if !req.source_map_include_sources {
                    sm.sources_content.clear();
                }
                if let Ok(bytes) = sm.json() {
                    success.source_map = String::from_utf8_lossy(&bytes).into_owned();
                }
            }
            let loaded_urls = result.loaded_urls().iter().map(|u| u.to_string()).collect();
            Ok(outbound_message::CompileResponse {
                loaded_urls,
                result: Some(compile_response::Result::Success(success)),
            })
        }
        Err(e) => Ok(build_failure_response(
            &ctx.host,
            &e,
            entrypoint.as_ref(),
            &highlight,
        )),
    }
}

// Embedded-proto → `CompileOptions` wiring; arity follows the request
// fields, a params struct would just rename them.
#[allow(clippy::too_many_arguments)]
fn base_options<'compile, 'parse>(
    arena: &'compile Bump,
    req: &inbound_message::CompileRequest,
    importers: Vec<Importer<'parse>>,
    functions: Vec<rust_sass::callable::Callable<'compile, 'parse>>,
    logger: Rc<dyn Logger>,
    style: OutputStyle,
    fatal: Vec<&'static Deprecation>,
    silence: Vec<&'static Deprecation>,
    future: Vec<&'static Deprecation>,
) -> CompileOptions<'compile, 'parse>
where
    'compile: 'parse,
    'parse: 'compile,
{
    CompileOptions {
        importers,
        functions,
        logger: Some(logger),
        quiet_deps: req.quiet_deps,
        source_map: req.source_map,
        include_source_map_sources: req.source_map_include_sources,
        style,
        charset: req.charset,
        verbose: req.verbose,
        alert_color: req.alert_color,
        alert_ascii: req.alert_ascii,
        silence_deprecations: silence,
        fatal_deprecations: fatal,
        future_deprecations: future,
        ..CompileOptions::new(arena)
    }
}

/// Decodes a proto `Importer` into a compiler `Importer`.
///
/// Matches Go: `CompilationDispatcher.decodeImporter`.
fn decode_importer<'compile, 'parse>(
    arena: &'compile Bump,
    ctx: Rc<HostContext>,
    imp: &compile_request::Importer,
) -> Result<Option<Importer<'parse>>, DecodeImporterError>
where
    'compile: 'parse,
{
    match &imp.importer {
        Some(compile_request::importer::Importer::Path(p)) => {
            check_no_non_canonical_scheme(imp)?;
            Ok(Some(Importer::new(
                arena,
                ImporterKind::Filesystem(FilesystemImporter::new(p, ctx.io.clone())),
            )))
        }
        Some(compile_request::importer::Importer::ImporterId(id)) => Ok(Some(Importer::new(
            arena,
            ImporterKind::User(Rc::new(
                HostImporter::new(ctx, *id, imp.non_canonical_scheme.clone())
                    .map_err(DecodeImporterError::Sass)?,
            )),
        ))),
        Some(compile_request::importer::Importer::FileImporterId(id)) => {
            check_no_non_canonical_scheme(imp)?;
            Ok(Some(Importer::new(
                arena,
                ImporterKind::User(Rc::new(FileImporter::new(ctx, *id))),
            )))
        }
        Some(compile_request::importer::Importer::NodePackageImporter(npi)) => {
            Ok(Some(Importer::new(
                arena,
                ImporterKind::NodePackage(NodePackageImporter::new(
                    &npi.entry_point_directory,
                    ctx.io.clone(),
                )),
            )))
        }
        None => {
            check_no_non_canonical_scheme(imp)?;
            Ok(None)
        }
    }
}

fn check_no_non_canonical_scheme(
    imp: &compile_request::Importer,
) -> Result<(), DecodeImporterError> {
    if imp.non_canonical_scheme.is_empty() {
        Ok(())
    } else {
        Err(DecodeImporterError::Protocol(params_error(
            "Importer.non_canonical_scheme may only be set along with Importer.importer.importer_id",
        )))
    }
}

/// Converts string deprecation IDs to `Deprecation`s, warning for unknown IDs
/// and expanding version strings when `support_versions` is true.
///
/// Matches Go: `CompilationDispatcher.parseDeprecationsOrWarn`.
fn parse_deprecations_or_warn(
    deprecations: &[String],
    support_versions: bool,
    logger: Rc<dyn Logger>,
) -> Vec<&'static Deprecation> {
    let mut result = Vec::new();
    for item in deprecations {
        if let Some(d) = deprecation::from_id(item) {
            result.push(d);
        } else if support_versions {
            let expanded = deprecation::for_version(item);
            if expanded.is_empty() {
                logger.warn(
                    &format!("Invalid deprecation id or version \"{item}\"."),
                    None,
                    None,
                );
            } else {
                result.extend(expanded);
            }
        } else {
            logger.warn(&format!("Invalid deprecation id \"{item}\"."), None, None);
        }
    }
    result
}

/// Builds a `CompileResponse` carrying a `CompileFailure` for `err`, prepending
/// `entrypoint` to the loaded URLs (Dart reports the entrypoint even on
/// failure).
fn build_failure_response(
    ctx: &HostContext,
    err: &SassError,
    entrypoint: Option<&SassUrl>,
    highlight: &rust_sass::common::source_span_highlighter::HighlightOptions,
) -> outbound_message::CompileResponse {
    let failure = build_failure(err, ctx.io.as_ref(), highlight);
    let mut loaded_urls: Vec<String> = err.loaded_urls().iter().map(|u| u.to_string()).collect();
    if let Some(ep) = entrypoint {
        let ep = ep.to_string();
        if !loaded_urls.contains(&ep) {
            loaded_urls.insert(0, ep);
        }
    }
    outbound_message::CompileResponse {
        loaded_urls,
        result: Some(compile_response::Result::Failure(failure)),
    }
}

#[cfg(test)]
#[cfg(feature = "async")]
pub(crate) fn test_host(rx: futures::channel::mpsc::UnboundedReceiver<Vec<u8>>) -> Rc<HostContext> {
    Rc::new(HostContext {
        compilation_id: 1,
        io: Rc::new(rust_sass::io::DefaultIo::new()),
        rx: Rc::new(RefCell::new(rx)),
        compile_context: rust_sass::compile_context::new_compile_context(),
        protocol_version: "3.2.0",
        compiler_version: COMPILER_VERSION,
        outbound_request_id: 0,
        writer: Arc::new(Mutex::new(Box::new(Vec::new()) as Box<dyn Write + Send>)),
        stderr: Arc::new(Mutex::new(Box::new(Vec::new()) as Box<dyn Write + Send>)),
    })
}

#[cfg(test)]
#[cfg(not(feature = "async"))]
pub(crate) fn test_host(rx: Receiver<Vec<u8>>) -> Rc<HostContext> {
    Rc::new(HostContext {
        compilation_id: 1,
        io: Rc::new(rust_sass::io::DefaultIo::new()),
        rx: Rc::new(RefCell::new(rx)),
        compile_context: rust_sass::compile_context::new_compile_context(),
        protocol_version: "3.2.0",
        compiler_version: COMPILER_VERSION,
        outbound_request_id: 0,
        writer: Arc::new(Mutex::new(Box::new(Vec::new()) as Box<dyn Write + Send>)),
        stderr: Arc::new(Mutex::new(Box::new(Vec::new()) as Box<dyn Write + Send>)),
    })
}

#[cfg(all(test, feature = "async"))]
pub(crate) fn test_context<'compile, 'parse>(
    rx: futures::channel::mpsc::UnboundedReceiver<Vec<u8>>,
    arena: &'compile Bump,
) -> Rc<CompilationContext<'compile, 'parse>>
where
    'compile: 'parse,
{
    Rc::new(CompilationContext {
        host: test_host(rx),
        functions: Rc::new(RefCell::new(OpaqueRegistry::<SassFunction<'parse>>::new())),
        mixins: Rc::new(RefCell::new(OpaqueRegistry::<SassMixin<'parse>>::new())),
        arena,
    })
}

#[cfg(all(test, not(feature = "async")))]
pub(crate) fn test_context<'compile, 'parse>(
    rx: Receiver<Vec<u8>>,
    arena: &'compile Bump,
) -> Rc<CompilationContext<'compile, 'parse>>
where
    'compile: 'parse,
{
    Rc::new(CompilationContext {
        host: test_host(rx),
        functions: Rc::new(RefCell::new(OpaqueRegistry::<SassFunction<'parse>>::new())),
        mixins: Rc::new(RefCell::new(OpaqueRegistry::<SassMixin<'parse>>::new())),
        arena,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedded_sass::inbound_message::canonicalize_response;
    use crate::embedded_sass::{inbound_message as im, outbound_message as om, InboundMessage};
    use crate::packet::serialize_packet;

    fn outbound_canonicalize_request() -> OutboundMessage {
        OutboundMessage {
            message: Some(om::Message::CanonicalizeRequest(om::CanonicalizeRequest {
                id: 0,
                importer_id: 3,
                url: "foo".into(),
                containing_url: None,
                from_import: false,
            })),
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn send_and_wait_returns_response() {
        #[cfg(feature = "async")]
        let (tx, rx) = futures::channel::mpsc::unbounded::<Vec<u8>>();
        #[cfg(not(feature = "async"))]
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let ctx = test_host(rx);
        let response = InboundMessage {
            message: Some(im::Message::CanonicalizeResponse(
                im::CanonicalizeResponse {
                    id: 0,
                    containing_url_unused: false,
                    result: Some(canonicalize_response::Result::Url("file:///a.scss".into())),
                },
            )),
        };
        #[cfg(feature = "async")]
        tx.unbounded_send(serialize_packet(1, &response)).unwrap();
        #[cfg(not(feature = "async"))]
        tx.send(serialize_packet(1, &response)).unwrap();
        let msg = ctx.send_and_wait(&outbound_canonicalize_request()).await;
        assert!(matches!(
            msg,
            Ok(im::Message::CanonicalizeResponse(r))
                if matches!(&r.result, Some(canonicalize_response::Result::Url(u)) if u == "file:///a.scss")
        ));
    }

    #[rust_sass_macros::maybe_test]
    async fn send_and_wait_rejects_compile_request() {
        #[cfg(feature = "async")]
        let (tx, rx) = futures::channel::mpsc::unbounded::<Vec<u8>>();
        #[cfg(not(feature = "async"))]
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let ctx = test_host(rx);
        let response = InboundMessage {
            message: Some(im::Message::CompileRequest(im::CompileRequest::default())),
        };
        #[cfg(feature = "async")]
        tx.unbounded_send(serialize_packet(1, &response)).unwrap();
        #[cfg(not(feature = "async"))]
        tx.send(serialize_packet(1, &response)).unwrap();
        let err = ctx.send_and_wait(&outbound_canonicalize_request()).await;
        assert!(matches!(err, Err(HostError::Protocol(_))));
    }

    #[rust_sass_macros::maybe_test]
    async fn send_and_wait_mailbox_closed() {
        #[cfg(feature = "async")]
        let (tx, rx) = futures::channel::mpsc::unbounded::<Vec<u8>>();
        #[cfg(not(feature = "async"))]
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        drop(tx); // sender dropped → next() returns None
        let ctx = test_host(rx);
        let err = ctx.send_and_wait(&outbound_canonicalize_request()).await;
        assert!(matches!(err, Err(HostError::MailboxClosed)));
    }
}
