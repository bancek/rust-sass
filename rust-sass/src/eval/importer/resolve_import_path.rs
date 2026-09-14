// Copyright 2017 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/importer/utils.dart (resolveImportPath + helpers)
// go-source: go/eval/resolve_import_path.go

use crate::common::pretty_uri::pretty_uri;
use std::path::Path;

use crate::common::exception::SassError;
use crate::io::{Io, IoError};
use crate::url::SassUrl;

fn io_err_to_sass(e: IoError) -> SassError {
    SassError::Script {
        message: e.to_string(),
        argument_name: None,
    }
}

/// Resolves an imported path using the same logic as the filesystem importer:
/// fills in extensions and partial prefixes and checks for a directory
/// default. Returns `None` when no file is found.
///
/// Rewritten from Dart's `resolveImportPath` (`importer/utils.dart`): with an
/// explicit extension, `@import` context additionally tries the `.import`
/// variant first; without one, `sass` and `scss` win over `css`, partials
/// (`_name`) sort before full names, and directories fall back to `index`.
#[rust_sass_macros::maybe_async]
pub async fn resolve_import_path(
    io: &dyn Io,
    path: &str,
    from_import: bool,
) -> Result<Option<String>, Box<SassError>> {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    if ext == "sass" || ext == "scss" || ext == "css" {
        if from_import {
            let without_ext = path.strip_suffix(&format!(".{}", ext)).unwrap_or(path);
            let import_path = format!("{}.import.{}", without_ext, ext);
            if let Some(result) = exactly_one(&try_path(io, &import_path).await?, io)? {
                return Ok(Some(result));
            }
        }
        return exactly_one(&try_path(io, path).await?, io);
    }

    if from_import {
        if let Some(result) = exactly_one(
            &try_path_with_extensions(io, &format!("{}.import", path)).await?,
            io,
        )? {
            return Ok(Some(result));
        }
    }

    if let Some(result) = exactly_one(&try_path_with_extensions(io, path).await?, io)? {
        return Ok(Some(result));
    }

    try_path_as_directory(io, path, from_import).await
}

// Returns `path` and/or the `_`-prefixed partial, if either exists.
// Matches Dart's `_tryPath` (importer/utils.dart).
#[rust_sass_macros::maybe_async]
async fn try_path(io: &dyn Io, path: &str) -> Result<Vec<String>, Box<SassError>> {
    let dir = Path::new(path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let base = Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let partial = Path::new(&dir).join(format!("_{}", base));
    let partial_str = partial.to_string_lossy().to_string();
    let mut result = Vec::new();
    if io
        .file_exists(Path::new(&partial_str))
        .await
        .map_err(io_err_to_sass)?
    {
        result.push(partial_str);
    }
    if io
        .file_exists(Path::new(path))
        .await
        .map_err(io_err_to_sass)?
    {
        result.push(path.to_string());
    }
    Ok(result)
}

// Like `try_path`, but checks `.sass`, `.scss`, then `.css` extensions.
// Matches Dart's `_tryPathWithExtensions` (importer/utils.dart).
#[rust_sass_macros::maybe_async]
async fn try_path_with_extensions(io: &dyn Io, path: &str) -> Result<Vec<String>, Box<SassError>> {
    let mut result = try_path(io, &format!("{}.sass", path)).await?;
    result.append(&mut try_path(io, &format!("{}.scss", path)).await?);
    if !result.is_empty() {
        return Ok(result);
    }
    try_path(io, &format!("{}.css", path)).await
}

// Returns the resolved `index` file when `path` is a directory.
// Matches Dart's `_tryPathAsDirectory` (importer/utils.dart).
#[rust_sass_macros::maybe_async]
async fn try_path_as_directory(
    io: &dyn Io,
    path: &str,
    from_import: bool,
) -> Result<Option<String>, Box<SassError>> {
    if !io
        .dir_exists(Path::new(path))
        .await
        .map_err(io_err_to_sass)?
    {
        return Ok(None);
    }

    if from_import {
        let index_import = Path::new(path).join("index.import");
        if let Some(result) = exactly_one(
            &try_path_with_extensions(io, &index_import.to_string_lossy()).await?,
            io,
        )? {
            return Ok(Some(result));
        }
    }

    let index_path = Path::new(path).join("index");
    exactly_one(
        &try_path_with_extensions(io, &index_path.to_string_lossy()).await?,
        io,
    )
}

// Returns the single path when exactly one candidate exists, `None` when
// there are none, and throws "It's not clear which file to import." when
// several match. Matches Dart's `_exactlyOne` (importer/utils.dart).
fn exactly_one(paths: &[String], io: &dyn Io) -> Result<Option<String>, Box<SassError>> {
    match paths.len() {
        0 => Ok(None),
        1 => Ok(Some(paths[0].clone())),
        _ => {
            let lines: Vec<String> = paths
                .iter()
                .map(|p| {
                    format!(
                        "  {}",
                        pretty_uri(
                            &SassUrl::parse(&format!("file://{}", p.replace('\\', "/"))).unwrap(),
                            io,
                        )
                    )
                })
                .collect();
            Err(Box::new(SassError::Script {
                message: format!(
                    "It's not clear which file to import. Found:\n{}",
                    lines.join("\n")
                ),
                argument_name: None,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::DefaultIo;

    #[rust_sass_macros::maybe_test]
    async fn resolve_exact_extension() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("style.scss");
        std::fs::write(&path, b"a {}").unwrap();
        let io = DefaultIo::new();

        let result = resolve_import_path(&io, &path.to_string_lossy(), false)
            .await
            .unwrap()
            .unwrap();
        let expected = io.canonicalize(&path).await.unwrap();
        let __awaited0 = io.canonicalize(Path::new(&result)).await.unwrap();

        assert_eq!(__awaited0, expected);
    }

    #[rust_sass_macros::maybe_test]
    async fn resolve_no_extension_finds_scss() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("style.scss");
        std::fs::write(&path, b"a {}").unwrap();
        let io = DefaultIo::new();
        let no_ext = dir.path().join("style");

        let result = resolve_import_path(&io, &no_ext.to_string_lossy(), false)
            .await
            .unwrap()
            .unwrap();
        let expected = io.canonicalize(&path).await.unwrap();
        let __awaited1 = io.canonicalize(Path::new(&result)).await.unwrap();

        assert_eq!(__awaited1, expected);
    }

    #[rust_sass_macros::maybe_test]
    async fn resolve_partial() {
        let dir = tempfile::tempdir().unwrap();
        let partial = dir.path().join("_partial.scss");
        std::fs::write(&partial, b"a {}").unwrap();
        let io = DefaultIo::new();

        let result = resolve_import_path(
            &io,
            &dir.path().join("partial.scss").to_string_lossy(),
            false,
        )
        .await
        .unwrap()
        .unwrap();
        let expected = io.canonicalize(&partial).await.unwrap();
        let __awaited2 = io.canonicalize(Path::new(&result)).await.unwrap();

        assert_eq!(__awaited2, expected);
    }

    #[rust_sass_macros::maybe_test]
    async fn resolve_import_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let import_file = dir.path().join("style.import.scss");
        let regular_file = dir.path().join("style.scss");
        std::fs::write(&import_file, b"a {}").unwrap();
        std::fs::write(&regular_file, b"b {}").unwrap();
        let io = DefaultIo::new();

        let result = resolve_import_path(&io, &dir.path().join("style").to_string_lossy(), true)
            .await
            .unwrap()
            .unwrap();
        let expected = io.canonicalize(&import_file).await.unwrap();
        let __awaited3 = io.canonicalize(Path::new(&result)).await.unwrap();

        assert_eq!(__awaited3, expected);
    }

    #[rust_sass_macros::maybe_test]
    async fn resolve_import_suffix_no_from_import() {
        let dir = tempfile::tempdir().unwrap();
        let import_file = dir.path().join("style.import.scss");
        let regular_file = dir.path().join("style.scss");
        std::fs::write(&import_file, b"a {}").unwrap();
        std::fs::write(&regular_file, b"b {}").unwrap();
        let io = DefaultIo::new();

        let result = resolve_import_path(&io, &dir.path().join("style").to_string_lossy(), false)
            .await
            .unwrap()
            .unwrap();
        let expected = io.canonicalize(&regular_file).await.unwrap();
        let __awaited4 = io.canonicalize(Path::new(&result)).await.unwrap();

        assert_eq!(__awaited4, expected);
    }

    #[rust_sass_macros::maybe_test]
    async fn resolve_index() {
        let dir = tempfile::tempdir().unwrap();
        let pkg = dir.path().join("mypackage");
        std::fs::create_dir(&pkg).unwrap();
        let index = pkg.join("_index.scss");
        std::fs::write(&index, b"a {}").unwrap();
        let io = DefaultIo::new();

        let result = resolve_import_path(&io, &pkg.to_string_lossy(), false)
            .await
            .unwrap()
            .unwrap();
        let expected = io.canonicalize(&index).await.unwrap();
        let __awaited5 = io.canonicalize(Path::new(&result)).await.unwrap();

        assert_eq!(__awaited5, expected);
    }

    #[rust_sass_macros::maybe_test]
    async fn resolve_not_found() {
        let io = DefaultIo::new();
        let result = resolve_import_path(&io, "/nonexistent/path/to/file", false)
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[rust_sass_macros::maybe_test]
    async fn resolve_ambiguous() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("style.scss");
        let b = dir.path().join("style.sass");
        std::fs::write(&a, b"a {}").unwrap();
        std::fs::write(&b, b"b").unwrap();
        let io = DefaultIo::new();

        let err = resolve_import_path(&io, &dir.path().join("style").to_string_lossy(), false)
            .await
            .unwrap_err();
        match *err {
            SassError::Script { message, .. } => {
                assert!(message.starts_with("It's not clear which file to import. Found:"));
                assert!(message.contains("style.sass"));
                assert!(message.contains("style.scss"));
            }
            _ => panic!("unexpected error variant"),
        }
    }
}
