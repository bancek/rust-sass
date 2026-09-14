// Copyright 2021 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/embedded/importer/file.dart
// go-source: go/embedded/importer_file.go

use std::fmt;
use std::rc::Rc;

#[cfg(feature = "async")]
use futures::future::LocalBoxFuture;
use rust_sass_macros;

use rust_sass::common::exception::SassResult;
use rust_sass::eval::importer::filesystem::FilesystemImporter;
use rust_sass::eval::importer::result::ImporterResult;
use rust_sass::eval::importer::{CanonicalizeContext, UserImporter};
use rust_sass::logger::NoOpWarnLogger;
use rust_sass::url::SassUrl;

use crate::compilation::HostContext;
use crate::embedded_sass::inbound_message::file_import_response;
use crate::embedded_sass::outbound_message;
use crate::embedded_sass::{inbound_message, OutboundMessage};
use crate::error::{script, OUTBOUND_REQUEST_ID};
use crate::importer_host::parse_absolute_url;

/// Asks the host to resolve imports in a simplified, file-system-centric way.
///
/// Matches Dart: `FileImporter`.
pub struct FileImporter {
    ctx: Rc<HostContext>,
    importer_id: u32,
    fs: FilesystemImporter,
}

impl FileImporter {
    pub fn new(ctx: Rc<HostContext>, importer_id: u32) -> Self {
        // Use `new_no_load_path`, not `new_cwd` (which sets a deprecated
        // load path that emits a spurious FS_IMPORTER_CWD deprecation).
        let fs = FilesystemImporter::new_no_load_path(ctx.io.clone());
        FileImporter {
            ctx,
            importer_id,
            fs,
        }
    }
}

impl fmt::Debug for FileImporter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileImporter")
            .field("importer_id", &self.importer_id)
            .finish_non_exhaustive()
    }
}

#[rust_sass_macros::maybe_async]
impl UserImporter for FileImporter {
    fn canonicalize<'a>(
        &'a self,
        url: &'a SassUrl,
        context: &'a mut CanonicalizeContext,
    ) -> LocalBoxFuture<'a, SassResult<Option<SassUrl>>> {
        if url.scheme() == "file" {
            let fs = &self.fs;
            return Box::pin(async move { fs.canonicalize(url, context, &NoOpWarnLogger).await });
        }

        let request = OutboundMessage {
            message: Some(outbound_message::Message::FileImportRequest(
                outbound_message::FileImportRequest {
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
        let fs = self.fs.clone();
        Box::pin(async move {
            let response = ctx.send_and_wait(&request).await?;
            let response = match response {
                inbound_message::Message::FileImportResponse(r) => r,
                _ => return Err(script("Expected FileImportResponse.")),
            };
            if !response.containing_url_unused {
                context.containing_url(); // mark as accessed
            }
            match response.result {
                Some(file_import_response::Result::FileUrl(u)) => {
                    let file_url = parse_absolute_url("The file importer", &u)?;
                    if file_url.scheme() != "file" {
                        return Err(script(&format!(
                            "The file importer must return a file: URL, was \"{file_url}\""
                        )));
                    }
                    fs.canonicalize(&file_url, context, &NoOpWarnLogger).await
                }
                Some(file_import_response::Result::Error(e)) => Err(script(&e)),
                None => Ok(None),
            }
        })
    }

    fn load<'a>(
        &'a self,
        url: &'a SassUrl,
    ) -> LocalBoxFuture<'a, SassResult<Option<ImporterResult>>> {
        let fs = &self.fs;
        Box::pin(async move { fs.load(url).await })
    }

    fn could_canonicalize(&self, _url: &SassUrl, _base_url: &SassUrl) -> bool {
        false
    }

    fn is_non_canonical_scheme(&self, scheme: &str) -> bool {
        scheme != "file"
    }
}
