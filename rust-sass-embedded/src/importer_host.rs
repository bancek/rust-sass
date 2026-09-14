// Copyright 2019 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/embedded/importer/host.dart
// go-source: go/embedded/importer_host.go

use crate::error::bogus_span;
use crate::syntax_from_proto;
use std::collections::HashSet;
use std::fmt;
use std::rc::Rc;

#[cfg(feature = "async")]
use futures::future::LocalBoxFuture;
use rust_sass_macros;

use rust_sass::common::exception::{SassError, SassResult};
use rust_sass::eval::importer::result::ImporterResult;
use rust_sass::eval::importer::{CanonicalizeContext, UserImporter};
use rust_sass::url::SassUrl;

use crate::compilation::HostContext;
use crate::embedded_sass::inbound_message::canonicalize_response;
use crate::embedded_sass::inbound_message::import_response;
use crate::embedded_sass::outbound_message;
use crate::embedded_sass::{inbound_message, OutboundMessage};
use crate::error::{script, OUTBOUND_REQUEST_ID};

/// Asks the host to resolve imports.
///
/// Matches Dart: `HostImporter`.
pub struct HostImporter {
    ctx: Rc<HostContext>,
    importer_id: u32,
    non_canonical_schemes: HashSet<String>,
}

impl HostImporter {
    pub fn new(
        ctx: Rc<HostContext>,
        importer_id: u32,
        non_canonical_schemes: Vec<String>,
    ) -> SassResult<Self> {
        for scheme in &non_canonical_schemes {
            if !is_valid_url_scheme(scheme) {
                return Err(Box::new(SassError::Sass {
                    message: format!("{scheme:?} isn't a valid URL scheme (for example \"file\")."),
                    span: bogus_span(),
                    cause: None,
                    loaded_urls: vec![],
                }));
            }
        }
        Ok(HostImporter {
            ctx,
            importer_id,
            non_canonical_schemes: non_canonical_schemes.into_iter().collect(),
        })
    }
}

impl fmt::Debug for HostImporter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostImporter")
            .field("importer_id", &self.importer_id)
            .finish_non_exhaustive()
    }
}

#[rust_sass_macros::maybe_async]
impl UserImporter for HostImporter {
    fn canonicalize<'a>(
        &'a self,
        url: &'a SassUrl,
        context: &'a mut CanonicalizeContext,
    ) -> LocalBoxFuture<'a, SassResult<Option<SassUrl>>> {
        let request = OutboundMessage {
            message: Some(outbound_message::Message::CanonicalizeRequest(
                outbound_message::CanonicalizeRequest {
                    id: OUTBOUND_REQUEST_ID,
                    importer_id: self.importer_id,
                    url: url.to_string(),
                    containing_url: context
                        .containing_url_without_marking()
                        .map(|u| u.to_string()),
                    from_import: context.from_import,
                },
            )),
        };
        let ctx = self.ctx.clone();
        Box::pin(async move {
            let response = ctx.send_and_wait(&request).await?;
            let response = match response {
                inbound_message::Message::CanonicalizeResponse(r) => r,
                _ => return Err(script("Expected CanonicalizeResponse.")),
            };
            if !response.containing_url_unused {
                context.containing_url(); // mark as accessed
            }
            match response.result {
                Some(canonicalize_response::Result::Url(u)) => {
                    let parsed = parse_absolute_url("The importer", &u)?;
                    Ok(Some(parsed))
                }
                Some(canonicalize_response::Result::Error(e)) => Err(script(&e)),
                None => Ok(None),
            }
        })
    }

    fn load<'a>(
        &'a self,
        url: &'a SassUrl,
    ) -> LocalBoxFuture<'a, SassResult<Option<ImporterResult>>> {
        let request = OutboundMessage {
            message: Some(outbound_message::Message::ImportRequest(
                outbound_message::ImportRequest {
                    id: OUTBOUND_REQUEST_ID,
                    importer_id: self.importer_id,
                    url: url.to_string(),
                },
            )),
        };
        let ctx = self.ctx.clone();
        Box::pin(async move {
            let response = ctx.send_and_wait(&request).await?;
            let response = match response {
                inbound_message::Message::ImportResponse(r) => r,
                _ => return Err(script("Expected ImportResponse.")),
            };
            match response.result {
                Some(import_response::Result::Success(success)) => {
                    // The host always sends `sourceMapUrl ?? ''`, so an empty
                    // string means "no source map URL" (same as None).
                    let source_map_url = success
                        .source_map_url
                        .as_deref()
                        .filter(|u| !u.is_empty())
                        .map(|u| parse_absolute_url("The importer", u))
                        .transpose()?;
                    let syntax = syntax_from_proto(success.syntax)?;
                    Ok(Some(ImporterResult::new(
                        success.contents.clone(),
                        syntax,
                        source_map_url,
                    )?))
                }
                Some(import_response::Result::Error(e)) => Err(script(&e)),
                None => Ok(None),
            }
        })
    }

    fn could_canonicalize(&self, _url: &SassUrl, _base_url: &SassUrl) -> bool {
        false
    }

    fn is_non_canonical_scheme(&self, scheme: &str) -> bool {
        self.non_canonical_schemes.contains(scheme)
    }
}

/// Parses `url_str` as a URL, erroring if it's invalid or relative.
///
/// Matches Go: `ImporterBase.ParseAbsoluteURL` / Dart: `parseAbsoluteUrl`.
pub(crate) fn parse_absolute_url(source: &str, url_str: &str) -> SassResult<SassUrl> {
    let parsed = SassUrl::parse(url_str)
        .map_err(|_| script(&format!("{source} must return a URL, was \"{url_str}\"")))?;
    if parsed.is_relative() {
        return Err(script(&format!(
            "{source} must return an absolute URL, was \"{parsed}\""
        )));
    }
    Ok(parsed)
}

/// Returns whether `scheme` is a valid URL scheme.
///
/// Matches Dart: `isValidUrlScheme`.
fn is_valid_url_scheme(scheme: &str) -> bool {
    !scheme.is_empty()
        && scheme.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '+' || c == '-' || c == '.'
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compilation::test_host;
    use crate::embedded_sass::inbound_message::{canonicalize_response, import_response};
    use crate::embedded_sass::{inbound_message as im, InboundMessage};
    use crate::packet::serialize_packet;
    #[cfg(not(feature = "async"))]
    use std::sync::mpsc::Sender;

    #[cfg(feature = "async")]
    fn scripted_ctx() -> (
        Rc<HostContext>,
        futures::channel::mpsc::UnboundedSender<Vec<u8>>,
    ) {
        #[cfg(feature = "async")]
        let (tx, rx) = futures::channel::mpsc::unbounded::<Vec<u8>>();
        #[cfg(not(feature = "async"))]
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        (test_host(rx), tx)
    }

    #[cfg(not(feature = "async"))]
    fn scripted_ctx() -> (Rc<HostContext>, Sender<Vec<u8>>) {
        #[cfg(feature = "async")]
        let (tx, rx) = futures::channel::mpsc::unbounded::<Vec<u8>>();
        #[cfg(not(feature = "async"))]
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        (test_host(rx), tx)
    }

    #[cfg(feature = "async")]
    fn push_response(tx: &futures::channel::mpsc::UnboundedSender<Vec<u8>>, msg: InboundMessage) {
        tx.unbounded_send(serialize_packet(1, &msg)).unwrap();
    }

    #[cfg(not(feature = "async"))]
    fn push_response(tx: &Sender<Vec<u8>>, msg: InboundMessage) {
        tx.send(serialize_packet(1, &msg)).unwrap();
    }

    #[test]
    fn non_canonical_scheme_validation() {
        let (ctx, _tx) = scripted_ctx();
        assert!(HostImporter::new(ctx.clone(), 1, vec!["http".into()]).is_ok());
        let err = HostImporter::new(ctx, 1, vec!["not a scheme".into()]).unwrap_err();
        assert!(err.message().contains("isn't a valid URL scheme"));
    }

    #[rust_sass_macros::maybe_test]
    async fn canonicalize_returns_url() {
        let (ctx, tx) = scripted_ctx();
        let importer = HostImporter::new(ctx, 3, vec![]).unwrap();
        push_response(
            &tx,
            InboundMessage {
                message: Some(im::Message::CanonicalizeResponse(
                    im::CanonicalizeResponse {
                        id: 0,
                        containing_url_unused: false,
                        result: Some(canonicalize_response::Result::Url("file:///a.scss".into())),
                    },
                )),
            },
        );
        let url = SassUrl::parse("foo").unwrap();
        let mut cc = CanonicalizeContext::new(None, false);
        let result = importer.canonicalize(&url, &mut cc).await;
        assert!(matches!(result, Ok(Some(u)) if u.as_str() == "file:///a.scss"));
        // containing_url_unused == false → containing_url was marked accessed.
        assert!(cc.was_containing_url_accessed());
    }

    #[rust_sass_macros::maybe_test]
    async fn canonicalize_rejects_relative() {
        let (ctx, tx) = scripted_ctx();
        let importer = HostImporter::new(ctx, 3, vec![]).unwrap();
        push_response(
            &tx,
            InboundMessage {
                message: Some(im::Message::CanonicalizeResponse(
                    im::CanonicalizeResponse {
                        id: 0,
                        containing_url_unused: false,
                        result: Some(canonicalize_response::Result::Url("relative/path".into())),
                    },
                )),
            },
        );
        let url = SassUrl::parse("foo").unwrap();
        let mut cc = CanonicalizeContext::new(None, false);
        let result = importer.canonicalize(&url, &mut cc).await;
        assert!(result.is_err());
    }

    #[rust_sass_macros::maybe_test]
    async fn canonicalize_error() {
        let (ctx, tx) = scripted_ctx();
        let importer = HostImporter::new(ctx, 3, vec![]).unwrap();
        push_response(
            &tx,
            InboundMessage {
                message: Some(im::Message::CanonicalizeResponse(
                    im::CanonicalizeResponse {
                        id: 0,
                        containing_url_unused: false,
                        result: Some(canonicalize_response::Result::Error("boom".into())),
                    },
                )),
            },
        );
        let url = SassUrl::parse("foo").unwrap();
        let mut cc = CanonicalizeContext::new(None, false);
        let result = importer.canonicalize(&url, &mut cc).await;
        assert!(matches!(result, Err(e) if e.message() == "boom"));
    }

    #[rust_sass_macros::maybe_test]
    async fn load_success() {
        let (ctx, tx) = scripted_ctx();
        let importer = HostImporter::new(ctx, 3, vec![]).unwrap();
        push_response(
            &tx,
            InboundMessage {
                message: Some(im::Message::ImportResponse(im::ImportResponse {
                    id: 0,
                    result: Some(import_response::Result::Success(
                        import_response::ImportSuccess {
                            contents: "a { b: c }".into(),
                            syntax: 0, // SCSS
                            source_map_url: None,
                        },
                    )),
                })),
            },
        );
        let url = SassUrl::parse("file:///a.scss").unwrap();
        let result = importer.load(&url).await;
        let result = result.unwrap().unwrap();
        assert_eq!(result.contents, "a { b: c }");
    }

    #[rust_sass_macros::maybe_test]
    async fn load_empty_source_map_url_is_none() {
        // The host always sends `sourceMapUrl ?? ''`, so an empty string must
        // be treated as "no source map URL" rather than parsed (and rejected).
        let (ctx, tx) = scripted_ctx();
        let importer = HostImporter::new(ctx, 3, vec![]).unwrap();
        push_response(
            &tx,
            InboundMessage {
                message: Some(im::Message::ImportResponse(im::ImportResponse {
                    id: 0,
                    result: Some(import_response::Result::Success(
                        import_response::ImportSuccess {
                            contents: "a { b: c }".into(),
                            syntax: 0, // SCSS
                            source_map_url: Some(String::new()),
                        },
                    )),
                })),
            },
        );
        let url = SassUrl::parse("file:///a.scss").unwrap();
        let result = importer.load(&url).await;
        let result = result.unwrap().unwrap();
        assert_eq!(result.contents, "a { b: c }");
        // An empty source_map_url is treated as absent, so the generated
        // data: URL fallback is used (not the empty string, which would error).
        assert!(
            result
                .source_map_url()
                .as_str()
                .starts_with("data:;charset=utf-8,"),
            "expected generated data: URL, got {}",
            result.source_map_url()
        );
    }
}
