// Copyright 2019 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/embedded/host_callable.dart
// go-source: go/embedded/host_callable.go

use std::rc::Rc;

#[cfg(feature = "async")]
use futures::future::LocalBoxFuture;

use rust_sass::ast::sass::parameter_list::ParameterList;
#[cfg(feature = "async")]
use rust_sass::callable::AsyncBuiltInCallback;
#[cfg(not(feature = "async"))]
use rust_sass::callable::SyncBuiltInCallback;
use rust_sass::callable::{BuiltInCallable, Callable, CallableKind};
use rust_sass::common::exception::{SassError, SassResult};
use rust_sass::common::file_span::FileSpan;
use rust_sass::common::source_span_file_source::FileSource;
use rust_sass::common::source_span_span_with_context::SourceSpanWithContext;
use rust_sass::parse::stylesheet_parse::parse_parameter_list;
use rust_sass::value::Value;
use rust_sass::Bump;

use crate::compilation::CompilationContext;
use crate::embedded_sass::outbound_message::function_call_request;
use crate::embedded_sass::outbound_message::{self, FunctionCallRequest};
use crate::embedded_sass::{inbound_message, OutboundMessage};
use crate::error::OUTBOUND_REQUEST_ID;
use crate::protofier::Protofier;

/// Returns a `Callable` that invokes a function defined on the host with the
/// given `signature`.
///
/// If `id` is passed, the function is called by ID (necessary for anonymous
/// functions defined on the host); otherwise by the name in the signature.
///
/// Never panics on an invalid signature — it returns a `Script` error, which
/// the protocol requires be treated as a compilation failure.
///
/// Matches Dart: `hostCallable`.
pub fn host_callable<'compile, 'parse>(
    ctx: Rc<CompilationContext<'compile, 'parse>>,
    signature: &str,
    id: Option<u32>,
    arena: &'compile Bump,
) -> SassResult<Callable<'compile, 'parse>>
where
    'compile: 'parse,
    'parse: 'compile,
{
    let (name, params) = parse_host_signature(signature, arena)?;
    let callback = build_callback(ctx, name.clone(), id);
    Ok(Callable::new(
        arena,
        CallableKind::BuiltIn(BuiltInCallable::new_async(name, params, callback)),
    ))
}

/// Parses a host function signature into a name and `ParameterList`.
///
/// Matches Dart's `ScssParser.parseSignature` (lib/src/parse/stylesheet.dart):
/// `identifier()` then `_parameterList()` then `expectDone()`, including
/// Dart's exact error messages (`Invalid signature "{sig}": {detail}`). Host
/// signatures may not contain whitespace adjacent to the name or parentheses.
fn parse_host_signature<'compile, 'parse>(
    sig: &str,
    arena: &'compile Bump,
) -> SassResult<(String, ParameterList<'parse>)>
where
    'compile: 'parse,
{
    let bytes = sig.as_bytes();
    let first = *bytes
        .first()
        .ok_or_else(|| signature_error(sig, "Expected identifier.", 0, arena))?;
    if !is_name_start(first) {
        return Err(signature_error(sig, "Expected identifier.", 0, arena));
    }
    let mut i = 1;
    while i < bytes.len() && is_name(bytes[i]) {
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b'(' {
        return Err(signature_error(sig, "expected \"(\".", i, arena));
    }
    let name = &sig[..i];
    let open = i;
    // Find the matching close paren (balanced, so default values may nest).
    let mut close = None;
    let mut depth = 1usize;
    for (j, &b) in bytes.iter().enumerate().skip(open + 1) {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(j);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close.ok_or_else(|| signature_error(sig, "expected \")\".", sig.len(), arena))?;
    // Dart: expectDone() — nothing (including whitespace) may follow the `)`.
    if close + 1 != sig.len() {
        return Err(signature_error(
            sig,
            "expected no more input.",
            close + 1,
            arena,
        ));
    }
    let params_str = &sig[open + 1..close];
    let params = parse_parameter_list(&format!("@function {name}({params_str}) {{"), "", arena)
        .map_err(|_| signature_error(sig, "expected \")\".", open + 1, arena))?;
    // Dart parses the signature over the raw string (`ScssParser(signature)`),
    // so the declaration span must be over `"{name}({params})"` (offset 0)
    // rather than the `@function ... {` wrapper, for byte-identical error
    // spans/contexts.
    let signature_text = format!("{name}({params_str})");
    let source = FileSource::new_in(arena, &signature_text, None);
    let prefix = "@function ".len();
    let rebased = FileSpan::new(
        Some(source),
        params.span.start_location().offset.saturating_sub(prefix),
        params.span.end_location().offset.saturating_sub(prefix),
    );
    let params = ParameterList::new(params.parameters, rebased, params.rest_parameter);
    Ok((name.to_string(), params))
}

/// Dart `isNameStart`: `_`, `-`, ASCII alpha, or non-ASCII (Sass allows
/// `-foo` and non-ASCII identifiers).
fn is_name_start(b: u8) -> bool {
    b == b'_' || b == b'-' || b.is_ascii_alphabetic() || b >= 0x80
}

fn is_name(b: u8) -> bool {
    is_name_start(b) || b.is_ascii_digit()
}

/// Builds Dart's signature parse error: a `SassException` whose span is a
/// zero-width span over the signature at the failure `offset` (Dart's
/// `parseSignature` error span, `text` empty and `context` the signature).
fn signature_error(sig: &str, detail: &str, offset: usize, arena: &Bump) -> Box<SassError> {
    let source = FileSource::new_in(arena, sig, None);
    let span = FileSpan::new(Some(source), offset, offset);
    let span = SourceSpanWithContext::from_file_span(&span).unwrap();
    Box::new(SassError::Sass {
        message: format!("Invalid signature \"{sig}\": {detail}"),
        span,
        cause: None,
        loaded_urls: vec![],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse<'compile, 'parse>(
        sig: &str,
        arena: &'compile Bump,
    ) -> SassResult<(String, ParameterList<'parse>)>
    where
        'compile: 'parse,
    {
        parse_host_signature(sig, arena)
    }

    #[test]
    fn valid_signatures() {
        let arena = Bump::new();
        let (name, params) = parse("my-func($a, $b: 1px)", &arena).unwrap();
        assert_eq!(name, "my-func");
        assert_eq!(params.parameters.len(), 2);

        let (name, _) = parse("foo()", &arena).unwrap();
        assert_eq!(name, "foo");

        let (name, _) = parse("foo_bar-baz($x)", &arena).unwrap();
        assert_eq!(name, "foo_bar-baz");
    }

    #[test]
    fn invalid_signatures() {
        let arena = Bump::new();
        assert!(parse("no-parens", &arena).is_err());
        assert!(parse("", &arena).is_err());
        assert!(parse("1bad($a)", &arena).is_err());
        assert!(parse("bad name($a)", &arena).is_err());
        assert!(parse("foo(", &arena).is_err());
        assert!(parse("($a)", &arena).is_err());
        // Whitespace adjacent to the signature is rejected (Dart parseSignature).
        assert!(parse(" foo()", &arena).is_err());
        assert!(parse("foo() ", &arena).is_err());
        assert!(parse("foo ()", &arena).is_err());
        assert!(parse("foo(arg)", &arena).is_err());
        assert!(parse("$foo()", &arena).is_err());
    }

    #[test]
    fn invalid_signature_messages_match_dart() {
        let arena = Bump::new();
        assert_eq!(
            parse(" foo()", &arena).unwrap_err().message(),
            "Invalid signature \" foo()\": Expected identifier."
        );
        assert_eq!(
            parse("foo() ", &arena).unwrap_err().message(),
            "Invalid signature \"foo() \": expected no more input."
        );
        assert_eq!(
            parse("foo ()", &arena).unwrap_err().message(),
            "Invalid signature \"foo ()\": expected \"(\"."
        );
    }
}

#[cfg(feature = "async")]
#[allow(clippy::items_after_test_module)]
fn build_callback<'compile, 'parse>(
    ctx: Rc<CompilationContext<'compile, 'parse>>,
    name: String,
    id: Option<u32>,
) -> AsyncBuiltInCallback<'compile, 'parse>
where
    'compile: 'parse,
    'parse: 'compile,
{
    Rc::new(
        move |_config, _state, args, _arena| -> LocalBoxFuture<'_, SassResult<Value<'parse>>> {
            let ctx = ctx.clone();
            let name = name.clone();
            Box::pin(async move {
                let mut protofier = Protofier::new(ctx.clone());
                let mut request = FunctionCallRequest {
                    id: OUTBOUND_REQUEST_ID,
                    identifier: Some(match id {
                        Some(fid) => function_call_request::Identifier::FunctionId(fid),
                        None => function_call_request::Identifier::Name(name.clone()),
                    }),
                    arguments: Vec::new(),
                };
                for a in &args {
                    request.arguments.push(protofier.protofy(a)?);
                }
                let resp = ctx
                    .host
                    .send_and_wait(&OutboundMessage {
                        message: Some(outbound_message::Message::FunctionCallRequest(request)),
                    })
                    .await?;
                let resp = match resp {
                    inbound_message::Message::FunctionCallResponse(r) => r,
                    _ => {
                        return Err(Box::new(SassError::Script {
                            message: "Expected FunctionCallResponse.".into(),
                            argument_name: None,
                        }))
                    }
                };
                match resp.result {
                    Some(inbound_message::function_call_response::Result::Success(_)) => {
                        protofier.deprotofy_response(&resp)
                    }
                    Some(inbound_message::function_call_response::Result::Error(e)) => {
                        Err(Box::new(SassError::Script {
                            message: e,
                            argument_name: None,
                        }))
                    }
                    None => Err(Box::new(SassError::Script {
                        message: "Missing mandatory field result.".into(),
                        argument_name: None,
                    })),
                }
            })
        },
    )
}

/// Sync-build twin: same closure shape, converted to a plain `Fn` returning
/// the value directly (the maybe_async machinery rewrites the closure's
/// `LocalBoxFuture` return type and strips the inner `.await`s).
#[cfg(not(feature = "async"))]
#[allow(clippy::items_after_test_module)]
#[rust_sass_macros::must_be_sync]
async fn build_callback<'compile, 'parse>(
    ctx: Rc<CompilationContext<'compile, 'parse>>,
    name: String,
    id: Option<u32>,
) -> SyncBuiltInCallback<'compile, 'parse>
where
    'compile: 'parse,
    'parse: 'compile,
{
    Rc::new(
        move |_config, _state, args, _arena| -> LocalBoxFuture<'_, SassResult<Value<'parse>>> {
            let ctx = ctx.clone();
            let name = name.clone();
            Box::pin(async move {
                let mut protofier = Protofier::new(ctx.clone());
                let mut request = FunctionCallRequest {
                    id: OUTBOUND_REQUEST_ID,
                    identifier: Some(match id {
                        Some(fid) => function_call_request::Identifier::FunctionId(fid),
                        None => function_call_request::Identifier::Name(name.clone()),
                    }),
                    arguments: Vec::new(),
                };
                for a in &args {
                    request.arguments.push(protofier.protofy(a)?);
                }
                let resp = ctx
                    .host
                    .send_and_wait(&OutboundMessage {
                        message: Some(outbound_message::Message::FunctionCallRequest(request)),
                    })
                    .await?;
                let resp = match resp {
                    inbound_message::Message::FunctionCallResponse(r) => r,
                    _ => {
                        return Err(Box::new(SassError::Script {
                            message: "Expected FunctionCallResponse.".into(),
                            argument_name: None,
                        }))
                    }
                };
                match resp.result {
                    Some(inbound_message::function_call_response::Result::Success(_)) => {
                        protofier.deprotofy_response(&resp)
                    }
                    Some(inbound_message::function_call_response::Result::Error(e)) => {
                        Err(Box::new(SassError::Script {
                            message: e,
                            argument_name: None,
                        }))
                    }
                    None => Err(Box::new(SassError::Script {
                        message: "Missing mandatory field result.".into(),
                        argument_name: None,
                    })),
                }
            })
        },
    )
}
