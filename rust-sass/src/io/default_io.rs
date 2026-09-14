// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/io/vm.dart + lib/src/io.dart
// go-source: go/sassio/default_io.go + go/sassio/default_io_canonicalize.go
//   + go/sassio/default_io_real_case_path.go

//! Real-filesystem [`Io`] + [`IoExt`] implementation.
//!
//! Contract docs live on [`Io`]/[`IoExt`] in `super` (`io/mod.rs`); this file
//! holds the `Platform`-query, stdout/stderr, UTF-8, and `SourceFile`-trace
//! mechanics. Matches Dart: `io/vm.dart` mechanics + `io.dart`
//! (`canonicalize`, `_realCasePath`).
//!
//! UTF-8 note: Dart's `readFile` decodes bytes inline and throws a
//! `SassException` (`"Invalid UTF-8."` with a `SourceFile` trace at the first
//! bad byte). [`DefaultIo::read_file`] returns raw bytes instead; decoding
//! (and that error) is the caller's job, done at the `compile` entry.
//! `watchDir` has no Rust counterpart (omit per split rule). `realpath`
//! (`resolveSymbolicLinksSync`) likewise has none: [`DefaultIo::canonicalize`]
//! is lexical and preserves symlink names. `modificationTime` is reshaped as
//! [`IoExt::stat`] returning [`super::IoMetadata`].

use crate::io::clean_path;
use crate::io::real_case_path;
use std::collections::HashMap;
use std::io::Error;
use std::io::ErrorKind;
use std::io::IsTerminal;
use std::io::Read;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[cfg(feature = "async")]
use futures::future::LocalBoxFuture;

use crate::io::{Io, IoError, IoErrorKind, IoExt, IoMetadata};

/// Real-filesystem [`Io`] + [`IoExt`] implementation.
///
/// Backed by `std::fs`. Case correction state (`real_case_path` results) is
/// cached per instance; the exit code is stored in a `Mutex` so `&self`
/// setters stay object-safe under `Rc<dyn Io>`.
#[derive(Debug)]
pub struct DefaultIo {
    /// Cache of `real_case_path` results. Matches Dart's `_realCaseCache`.
    real_case_cache: Mutex<HashMap<String, String>>,
    /// Process exit code. Matches Dart's re-exported `dart:io exitCode`.
    exit_code: Mutex<i32>,
}

impl DefaultIo {
    /// Creates a `DefaultIo` with an empty case cache and exit code `0`.
    pub fn new() -> Self {
        DefaultIo {
            real_case_cache: Mutex::new(HashMap::new()),
            exit_code: Mutex::new(0),
        }
    }
}

impl Default for DefaultIo {
    fn default() -> Self {
        Self::new()
    }
}

/// Maps an OS error to its [`IoErrorKind`] category.
fn kind_from_io(err: &Error) -> IoErrorKind {
    match err.kind() {
        ErrorKind::NotFound => IoErrorKind::NotFound,
        ErrorKind::PermissionDenied => IoErrorKind::Permission,
        ErrorKind::AlreadyExists => IoErrorKind::AlreadyExists,
        _ => IoErrorKind::Other,
    }
}

/// Wraps an OS error as an [`IoError`] carrying `path`, mirroring how Dart's
/// `FileSystemException` always pairs `message` with `path`.
fn new_fse(err: Error, path: &Path) -> IoError {
    IoError {
        message: err.to_string(),
        kind: kind_from_io(&err),
        path: Some(path.to_string_lossy().into_owned()),
    }
}

/// Uppercases a Windows drive-letter prefix (`c:\…` → `C:\…`), matching
/// Dart's `_realCasePath` (`io.dart`). Pure string logic, factored out of the
/// Windows-only call site in [`DefaultIo::real_case_path`] so it is testable
/// on every platform.
// Used only on Windows outside tests (the sole non-test call site is
// `#[cfg(target_os = "windows")]`).
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn uppercase_drive_prefix(path: &str) -> String {
    if path.len() >= 2 && path.as_bytes().get(1) == Some(&b':') {
        path[..1].to_uppercase() + &path[1..]
    } else {
        path.to_string()
    }
}

impl DefaultIo {
    /// Returns `path` with each component's case corrected to match the real
    /// filesystem, via the shared [`super::real_case_path`] helper.
    ///
    /// Matches Dart's `_realCasePath` (`io.dart`): drive letters are
    /// uppercased on Windows; symlink names are preserved because matching
    /// goes through directory entries, not resolution. Results are cached
    /// per instance.
    fn real_case_path(&self, path: &str) -> String {
        if let Some(cached) = self.real_case_cache.lock().unwrap().get(path) {
            return cached.clone();
        }

        #[cfg(target_os = "windows")]
        let path = uppercase_drive_prefix(path);
        #[cfg(not(target_os = "windows"))]
        let path = path.to_string();

        let mut cache = self.real_case_cache.lock().unwrap();
        real_case_path(&path, &mut cache, |dir| {
            std::fs::read_dir(dir).ok().map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .map(|e| e.path().to_string_lossy().into_owned())
                    .collect()
            })
        })
    }
}

impl DefaultIo {
    // Recursive helper behind `list_dir`: pushes file paths (never
    // directories) into `result`. Matches Dart's `listDir`
    // (`.whereType<File>()` filter); on read failure throws an `IoError`
    // naming `root`, mirroring Dart's enclosing-path `FileSystemException`.
    fn walk_dir(
        &self,
        root: &Path,
        current: &Path,
        recursive: bool,
        result: &mut Vec<String>,
    ) -> Result<(), IoError> {
        let entries = std::fs::read_dir(current).map_err(|e| new_fse(e, root))?;
        for entry in entries {
            let entry = entry.map_err(|e| new_fse(e, root))?;
            let file_type = entry.file_type().map_err(|e| new_fse(e, root))?;
            if file_type.is_dir() {
                if recursive {
                    self.walk_dir(root, &entry.path(), recursive, result)?;
                }
            } else {
                result.push(entry.path().to_string_lossy().into_owned());
            }
        }
        Ok(())
    }
}

#[rust_sass_macros::maybe_async]
impl Io for DefaultIo {
    /// Reads the file at `path` as raw bytes. Matches Dart's `readFile`
    /// minus UTF-8 decoding (done by the `compile` entry).
    fn read_file<'a>(&'a self, path: &'a Path) -> LocalBoxFuture<'a, Result<Vec<u8>, IoError>> {
        Box::pin(async move { std::fs::read(path).map_err(|e| new_fse(e, path)) })
    }

    /// Returns whether a file at `path` exists. Unlike `std::path::Path::exists`
    /// (bare `bool`), non-ENOENT errors (e.g. `EACCES`) surface as `Err`.
    fn file_exists<'a>(&'a self, path: &'a Path) -> LocalBoxFuture<'a, Result<bool, IoError>> {
        Box::pin(async move {
            match std::fs::metadata(path) {
                Ok(m) => Ok(m.is_file()),
                Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
                Err(e) => Err(new_fse(e, path)),
            }
        })
    }

    /// Returns whether a dir at `path` exists (`Err` on non-ENOENT failures).
    fn dir_exists<'a>(&'a self, path: &'a Path) -> LocalBoxFuture<'a, Result<bool, IoError>> {
        Box::pin(async move {
            match std::fs::metadata(path) {
                Ok(m) => Ok(m.is_dir()),
                Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
                Err(e) => Err(new_fse(e, path)),
            }
        })
    }

    /// Returns whether a symlink at `path` exists. Uses `symlink_metadata` so
    /// broken links still count, matching Dart's `Link.existsSync`.
    fn link_exists<'a>(&'a self, path: &'a Path) -> LocalBoxFuture<'a, Result<bool, IoError>> {
        Box::pin(async move {
            match std::fs::symlink_metadata(path) {
                Ok(m) => Ok(m.file_type().is_symlink()),
                Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
                Err(e) => Err(new_fse(e, path)),
            }
        })
    }

    /// Returns the canonical form of `path`: absolute + normalized, with case
    /// corrected via [`DefaultIo::real_case_path`] on case-insensitive
    /// filesystems (macOS/Windows), matching Dart's `canonicalize`.
    fn canonicalize<'a>(&'a self, path: &'a Path) -> LocalBoxFuture<'a, Result<String, IoError>> {
        Box::pin(async move {
            let abs = if path.is_absolute() {
                PathBuf::from(path)
            } else {
                std::env::current_dir()
                    .map_err(|e| new_fse(e, path))?
                    .join(path)
            };

            // Matches Dart's canonicalize: `p.normalize(p.absolute(path))`, then
            // on case-insensitive filesystems `_realCasePath`. This is LEXICAL —
            // it never resolves symlinks (unlike std::fs::canonicalize), so
            // e.g. `/var/...` stays `/var/...` rather than becoming
            // `/private/var/...`.
            let normalized = clean_path(&abs.to_string_lossy());
            if cfg!(any(target_os = "macos", target_os = "windows")) {
                Ok(self.real_case_path(&normalized))
            } else {
                Ok(normalized)
            }
        })
    }

    /// Returns the process CWD, falling back to `"/"` when undeterminable.
    /// Stays sync deliberately: it is the only `Io` call in the
    /// error-formatting path.
    fn current_dir(&self) -> String {
        std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "/".to_string())
    }

    /// Whether the process runs on Windows. Matches Dart's
    /// `Platform.isWindows`.
    fn is_windows(&self) -> bool {
        cfg!(target_os = "windows")
    }

    /// Whether the process runs on macOS. Matches Dart's
    /// `Platform.isMacOS`.
    fn is_macos(&self) -> bool {
        cfg!(target_os = "macos")
    }

    /// Whether stdout is a terminal supporting ANSI escapes. Returns `false`
    /// for `TERM=dumb`; otherwise probes `stdout().is_terminal()`. (Dart
    /// additionally distrusts `supportsAnsiEscapes` off Windows; here the
    /// terminal probe is the whole check.)
    fn supports_ansi_escapes(&self) -> bool {
        if std::env::var("TERM").is_ok_and(|v| v == "dumb") {
            return false;
        }
        std::io::stdout().is_terminal()
    }
}

#[rust_sass_macros::maybe_async]
impl IoExt for DefaultIo {
    /// Writes `contents` to `path`. Matches Dart's `writeFile` minus UTF-8
    /// encoding (the caller passes bytes directly).
    fn write_file<'a>(
        &'a self,
        path: &'a Path,
        contents: &'a [u8],
    ) -> LocalBoxFuture<'a, Result<(), IoError>> {
        Box::pin(async move { std::fs::write(path, contents).map_err(|e| new_fse(e, path)) })
    }

    /// Deletes the file at `path`. Matches Dart's `deleteFile` (`deleteSync`).
    fn delete_file<'a>(&'a self, path: &'a Path) -> LocalBoxFuture<'a, Result<(), IoError>> {
        Box::pin(async move { std::fs::remove_file(path).map_err(|e| new_fse(e, path)) })
    }

    /// Creates `path` and ancestors. Matches Dart's `ensureDir`
    /// (`createSync(recursive: true)`).
    fn ensure_dir<'a>(&'a self, path: &'a Path) -> LocalBoxFuture<'a, Result<(), IoError>> {
        Box::pin(async move { std::fs::create_dir_all(path).map_err(|e| new_fse(e, path)) })
    }

    /// Lists files (not sub-directories) under `path`; with `recursive`,
    /// descends transitively. Matches Dart's `listDir` `whereType<File>`
    /// filter.
    fn list_dir<'a>(
        &'a self,
        path: &'a Path,
        recursive: bool,
    ) -> LocalBoxFuture<'a, Result<Vec<String>, IoError>> {
        Box::pin(async move {
            let mut result = Vec::new();
            self.walk_dir(path, path, recursive, &mut result)?;
            Ok(result)
        })
    }

    /// Reads stdin until close. Returns bytes (Dart decodes via
    /// `systemEncoding`); a read error carries no path.
    fn read_stdin(&self) -> LocalBoxFuture<'_, Result<Vec<u8>, IoError>> {
        Box::pin(async move {
            let mut buf = Vec::new();
            std::io::stdin()
                .lock()
                .read_to_end(&mut buf)
                .map_err(|e: Error| IoError {
                    message: e.to_string(),
                    kind: kind_from_io(&e),
                    path: None,
                })?;
            Ok(buf)
        })
    }

    /// Returns the modification time of `path` as [`IoMetadata`]. Reshaped
    /// from Dart's `modificationTime` (which throws `FileSystemException`
    /// on missing files); throws [`IoError`] here instead.
    fn stat<'a>(&'a self, path: &'a Path) -> LocalBoxFuture<'a, Result<IoMetadata, IoError>> {
        Box::pin(async move {
            std::fs::metadata(path)
                .map(IoMetadata::from)
                .map_err(|e| new_fse(e, path))
        })
    }

    /// Gets the process exit code. Matches Dart's re-exported `exitCode`.
    fn exit_code(&self) -> i32 {
        *self.exit_code.lock().unwrap()
    }

    /// Sets the process exit code.
    fn set_exit_code(&self, code: i32) {
        *self.exit_code.lock().unwrap() = code;
    }

    /// Writes `message` (no trailing newline) to stdout. Rust-only helper
    /// with no Dart counterpart.
    fn print_output(&self, message: &str) {
        let _ = std::io::stdout().write_all(message.as_bytes());
    }

    /// Prints `message` plus a newline to stdout. Matches Dart's `safePrint`
    /// (which delegates to `print`); thread-safe.
    fn safe_print(&self, message: &str) {
        let _ = std::io::stdout().write_all(message.as_bytes());
        let _ = std::io::stdout().write_all(b"\n");
    }

    /// Prints `message` plus a newline to stderr. Matches Dart's
    /// `printError` (`stderr.writeln`); thread-safe.
    fn print_error(&self, message: &str) {
        let _ = std::io::stderr().write_all(message.as_bytes());
        let _ = std::io::stderr().write_all(b"\n");
    }

    /// Returns the environment variable `name`, or `None` if unset.
    /// Matches Dart's `getEnvironmentVariable` (`Platform.environment`).
    fn get_environment_variable(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    /// Whether stdout is an interactive terminal. Matches Dart's
    /// `hasTerminal` (`stdout.hasTerminal`).
    fn has_terminal(&self) -> bool {
        std::io::stdout().is_terminal()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::clean_path;
    #[cfg(not(any(unix, windows)))]
    use std::io::Error;
    use std::path::Path;
    use std::path::MAIN_SEPARATOR;

    /// Creates a file symlink for tests, degrading gracefully where
    /// unsupported. `std::os::unix::fs::symlink` does not exist on Windows
    /// (whose symlinks additionally need privileges), so the platform shim
    /// keeps `--all-targets` compiling for every target; callers return
    /// early on `Err`, matching the existing skip pattern.
    #[cfg(unix)]
    fn symlink_test_target(
        target: impl AsRef<Path>,
        link: impl AsRef<Path>,
    ) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(windows)]
    fn symlink_test_target(
        target: impl AsRef<Path>,
        link: impl AsRef<Path>,
    ) -> std::io::Result<()> {
        std::os::windows::fs::symlink_file(target, link)
    }

    #[cfg(not(any(unix, windows)))]
    fn symlink_test_target(
        target: impl AsRef<Path>,
        link: impl AsRef<Path>,
    ) -> std::io::Result<()> {
        let _ = (target, link);
        Err(Error::new(ErrorKind::Unsupported, "symlinks unsupported"))
    }

    #[rust_sass_macros::maybe_test]
    async fn read_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        std::fs::write(&path, b"hello").unwrap();

        let io = DefaultIo::new();
        let data = io.read_file(&path).await.unwrap();
        assert_eq!(data, b"hello");
        assert_eq!(std::str::from_utf8(&data).unwrap(), "hello");
    }

    #[rust_sass_macros::maybe_test]
    async fn read_file_not_found() {
        let io = DefaultIo::new();
        let err = io
            .read_file(Path::new("/nonexistent-file-for-test"))
            .await
            .unwrap_err();
        assert_eq!(err.path.as_deref(), Some("/nonexistent-file-for-test"));
        assert!(err.is_not_found());
        assert!(!err.is_permission());
        assert!(!err.is_already_exists());
    }

    #[rust_sass_macros::maybe_test]
    async fn stat() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stat.txt");
        std::fs::write(&path, b"x").unwrap();

        let io = DefaultIo::new();
        let info = io.stat(&path).await.unwrap();
        assert!(info.modified().is_ok());
    }

    #[rust_sass_macros::maybe_test]
    async fn stat_not_found() {
        let io = DefaultIo::new();
        let err = io
            .stat(Path::new("/nonexistent-file-for-test"))
            .await
            .unwrap_err();
        assert_eq!(err.path.as_deref(), Some("/nonexistent-file-for-test"));
        assert!(err.is_not_found());
    }

    #[rust_sass_macros::maybe_test]
    async fn uppercase_drive_prefix_uppercases_drive_letter() {
        assert_eq!(uppercase_drive_prefix("c:\\foo\\bar"), "C:\\foo\\bar");
        assert_eq!(uppercase_drive_prefix("C:\\foo"), "C:\\foo");
        assert_eq!(uppercase_drive_prefix("c:/foo"), "C:/foo");
        assert_eq!(uppercase_drive_prefix("relative/path"), "relative/path");
        assert_eq!(
            uppercase_drive_prefix("\\\\server\\share"),
            "\\\\server\\share"
        );
        assert_eq!(uppercase_drive_prefix("x"), "x");
        assert_eq!(uppercase_drive_prefix(""), "");
    }

    #[rust_sass_macros::maybe_test]
    async fn canonicalize_absolute() {
        let io = DefaultIo::new();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.scss");
        std::fs::write(&path, b"x").unwrap();

        let result = io.canonicalize(&path).await.unwrap();
        assert!(Path::new(&result).is_absolute());
    }

    #[rust_sass_macros::maybe_test]
    async fn canonicalize_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        let link = dir.path().join("alink");
        std::fs::write(&target, b"").unwrap();
        if symlink_test_target(&target, &link).is_err() {
            return; // symlinks not supported
        }

        let io = DefaultIo::new();
        let result = io.canonicalize(&link).await.unwrap();
        // Dart's canonicalize is lexical and preserves symlink names; it does
        // NOT resolve `link` to `target`.
        let expected = clean_path(&link.to_string_lossy());
        assert_eq!(result, expected);
    }

    #[rust_sass_macros::maybe_test]
    async fn canonicalize_case() {
        let io = DefaultIo::new();
        let dir = tempfile::tempdir().unwrap();

        let real_name = "MixedCase_SCSS.scss";
        let real_path = dir.path().join(real_name);
        std::fs::write(&real_path, b"x").unwrap();

        let wrong_case = dir.path().join("mixedcase_scss.scss");
        if std::fs::metadata(&wrong_case).is_err() {
            return; // case-sensitive filesystem; case correction doesn't apply
        }
        let result = io.canonicalize(&wrong_case).await.unwrap();

        assert!(result.ends_with(real_name));
    }

    #[rust_sass_macros::maybe_test]
    async fn canonicalize_nonexistent() {
        let io = DefaultIo::new();
        let result = io
            .canonicalize(Path::new("/nonexistent-path-for-test/foo.scss"))
            .await
            .unwrap();
        assert!(Path::new(&result).is_absolute());
    }

    #[rust_sass_macros::maybe_test]
    async fn write_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("write.txt");

        let io = DefaultIo::new();
        io.write_file(&path, b"hello").await.unwrap();

        let data = io.read_file(&path).await.unwrap();
        assert_eq!(data, b"hello");
        assert_eq!(std::str::from_utf8(&data).unwrap(), "hello");
    }

    #[rust_sass_macros::maybe_test]
    async fn write_file_permission_denied() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nope.txt");
        std::fs::write(&path, b"x").unwrap();

        // Read-only on the FILE itself: a read-only directory still accepts
        // new files on Windows (the attribute is advisory there), so
        // dir-level readonly is a unix-only construction.
        let orig = std::fs::metadata(&path).unwrap().permissions();
        let mut perms = orig.clone();
        perms.set_readonly(true);
        std::fs::set_permissions(&path, perms).unwrap();

        let io = DefaultIo::new();
        let result = io.write_file(&path, b"x").await;

        // Reset first: tempdir cleanup cannot delete read-only files on
        // Windows.
        std::fs::set_permissions(&path, orig).unwrap();

        let err = result.unwrap_err();
        assert_eq!(err.path.as_deref(), Some(path.to_str().unwrap()));
        assert!(err.is_permission());
    }

    #[rust_sass_macros::maybe_test]
    async fn delete_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("del.txt");
        std::fs::write(&path, b"x").unwrap();

        let io = DefaultIo::new();
        io.delete_file(&path).await.unwrap();
        let __awaited6 = !io.file_exists(&path).await.unwrap();

        assert!(__awaited6);
    }

    #[rust_sass_macros::maybe_test]
    async fn delete_file_not_found() {
        let io = DefaultIo::new();
        let err = io
            .delete_file(Path::new("/nonexistent-file-for-test"))
            .await
            .unwrap_err();
        assert_eq!(err.path.as_deref(), Some("/nonexistent-file-for-test"));
        assert!(err.is_not_found());
    }

    #[rust_sass_macros::maybe_test]
    async fn file_exists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("exists.txt");
        std::fs::write(&path, b"x").unwrap();

        let io = DefaultIo::new();
        let __awaited7 = io.file_exists(&path).await.unwrap();

        assert!(__awaited7);
    }

    #[rust_sass_macros::maybe_test]
    async fn file_exists_not_found() {
        let io = DefaultIo::new();
        let __awaited8 = !io
            .file_exists(Path::new("/nonexistent-file-for-test"))
            .await
            .unwrap();

        assert!(__awaited8);
    }

    #[rust_sass_macros::maybe_test]
    async fn file_exists_is_dir() {
        let dir = tempfile::tempdir().unwrap();
        let io = DefaultIo::new();
        let __awaited9 = !io.file_exists(dir.path()).await.unwrap();

        assert!(__awaited9);
    }

    #[rust_sass_macros::maybe_test]
    async fn dir_exists() {
        let dir = tempfile::tempdir().unwrap();
        let io = DefaultIo::new();
        let __awaited10 = io.dir_exists(dir.path()).await.unwrap();

        assert!(__awaited10);
    }

    #[rust_sass_macros::maybe_test]
    async fn dir_exists_not_found() {
        let io = DefaultIo::new();
        let __awaited11 = !io
            .dir_exists(Path::new("/nonexistent-dir-for-test"))
            .await
            .unwrap();

        assert!(__awaited11);
    }

    #[rust_sass_macros::maybe_test]
    async fn dir_exists_is_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.txt");
        std::fs::write(&path, b"x").unwrap();

        let io = DefaultIo::new();
        let __awaited12 = !io.dir_exists(&path).await.unwrap();

        assert!(__awaited12);
    }

    #[rust_sass_macros::maybe_test]
    async fn link_exists() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("link_target");
        let link_path = dir.path().join("the_link");
        std::fs::write(&target, b"x").unwrap();
        if symlink_test_target(&target, &link_path).is_err() {
            return; // symlinks not supported
        }

        let io = DefaultIo::new();
        let __awaited13 = io.link_exists(&link_path).await.unwrap();

        assert!(__awaited13);
    }

    #[rust_sass_macros::maybe_test]
    async fn link_exists_not_found() {
        let io = DefaultIo::new();
        let __awaited14 = !io
            .link_exists(Path::new("/nonexistent-link-for-test"))
            .await
            .unwrap();

        assert!(__awaited14);
    }

    #[rust_sass_macros::maybe_test]
    async fn link_exists_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("regular.txt");
        std::fs::write(&path, b"x").unwrap();

        let io = DefaultIo::new();
        let __awaited15 = !io.link_exists(&path).await.unwrap();

        assert!(__awaited15);
    }

    #[rust_sass_macros::maybe_test]
    async fn ensure_dir() {
        let dir = tempfile::tempdir().unwrap();
        let new_dir = dir.path().join("newdir");

        let io = DefaultIo::new();
        io.ensure_dir(&new_dir).await.unwrap();
        let __awaited16 = io.dir_exists(&new_dir).await.unwrap();

        assert!(__awaited16);
    }

    #[rust_sass_macros::maybe_test]
    async fn ensure_dir_nested() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b").join("c");

        let io = DefaultIo::new();
        io.ensure_dir(&nested).await.unwrap();
        let __awaited17 = io.dir_exists(&nested).await.unwrap();

        assert!(__awaited17);
    }

    #[rust_sass_macros::maybe_test]
    async fn list_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"a").unwrap();
        std::fs::write(dir.path().join("b.txt"), b"b").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();

        let io = DefaultIo::new();
        let files = io.list_dir(dir.path(), false).await.unwrap();
        assert_eq!(files.len(), 2);
    }

    #[rust_sass_macros::maybe_test]
    async fn list_dir_empty() {
        let dir = tempfile::tempdir().unwrap();
        let io = DefaultIo::new();
        let files = io.list_dir(dir.path(), false).await.unwrap();
        assert_eq!(files.len(), 0);
    }

    #[rust_sass_macros::maybe_test]
    async fn list_dir_recursive() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("top.txt"), b"t").unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("nested.txt"), b"n").unwrap();

        let io = DefaultIo::new();
        let files = io.list_dir(dir.path(), true).await.unwrap();
        assert_eq!(files.len(), 2);
    }

    #[rust_sass_macros::maybe_test]
    async fn list_dir_not_found() {
        let io = DefaultIo::new();
        let err = io
            .list_dir(Path::new("/nonexistent-dir-for-test"), false)
            .await
            .unwrap_err();
        assert_eq!(err.path.as_deref(), Some("/nonexistent-dir-for-test"));
        assert!(err.is_not_found());
    }

    #[test]
    fn get_environment_variable_missing() {
        let io = DefaultIo::new();
        assert!(io
            .get_environment_variable("__NONEXISTENT_ENV_VAR_FOR_TESTS__")
            .is_none());
    }

    #[test]
    fn exit_code() {
        let io = DefaultIo::new();
        assert_eq!(io.exit_code(), 0);
    }

    #[test]
    fn exit_code_set_get() {
        let io = DefaultIo::new();
        io.set_exit_code(42);
        assert_eq!(io.exit_code(), 42);
    }

    #[test]
    fn is_windows() {
        let io = DefaultIo::new();
        if cfg!(target_os = "windows") {
            assert!(io.is_windows());
        } else {
            assert!(!io.is_windows());
        }
    }

    #[test]
    fn is_macos() {
        let io = DefaultIo::new();
        if cfg!(target_os = "macos") {
            assert!(io.is_macos());
        } else {
            assert!(!io.is_macos());
        }
    }

    #[rust_sass_macros::maybe_test]
    async fn canonicalize_relative() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("relative.scss");
        std::fs::write(&path, b"x").unwrap();

        let io = DefaultIo::new();
        let result = io.canonicalize(&path).await.unwrap();
        assert!(Path::new(&result).is_absolute());
    }

    #[rust_sass_macros::maybe_test]
    async fn canonicalize_dot_dot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("style.scss");
        std::fs::write(&path, b"x").unwrap();

        let input = dir.path().join("subdir").join("..").join("style.scss");

        let io = DefaultIo::new();
        let result = io.canonicalize(&input).await.unwrap();
        assert!(Path::new(&result).is_absolute());
    }

    #[rust_sass_macros::maybe_test]
    async fn canonicalize_broken_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let broken = dir.path().join("broken_link");
        if symlink_test_target("/nonexistent/target", &broken).is_err() {
            return; // symlinks not supported
        }

        let io = DefaultIo::new();
        let result = io.canonicalize(&broken).await.unwrap();
        assert!(Path::new(&result).is_absolute());
    }

    #[rust_sass_macros::maybe_test]
    async fn canonicalize_symlink_chain() {
        let dir = tempfile::tempdir().unwrap();
        let final_path = dir.path().join("final.scss");
        std::fs::write(&final_path, b"x").unwrap();
        let link1 = dir.path().join("chain1");
        let link2 = dir.path().join("chain2.scss");
        if symlink_test_target(&final_path, &link1).is_err()
            || symlink_test_target(&link1, &link2).is_err()
        {
            return; // symlinks not supported
        }

        let io = DefaultIo::new();
        let result = io.canonicalize(&link2).await.unwrap();
        // Dart's canonicalize preserves symlink names rather than resolving
        // through the chain.
        let expected = clean_path(&link2.to_string_lossy());
        assert_eq!(result, expected);
    }

    #[rust_sass_macros::maybe_test]
    async fn canonicalize_preserves_symlinked_dir() {
        // Mirrors the macOS `/var` -> `/private/var` situation: a symlinked
        // directory component must be kept as-is (Dart semantics), not resolved
        // to its target (std::fs::canonicalize behavior).
        let dir = tempfile::tempdir().unwrap();
        let real_sub = dir.path().join("real_sub");
        std::fs::create_dir(&real_sub).unwrap();
        let path = real_sub.join("style.scss");
        std::fs::write(&path, b"x").unwrap();
        let link_sub = dir.path().join("link_sub");
        if symlink_test_target(&real_sub, &link_sub).is_err() {
            return; // symlinks not supported
        }

        let input = link_sub.join("style.scss");
        let io = DefaultIo::new();
        let result = io.canonicalize(&input).await.unwrap();
        // Separator-aware: `clean_path` joins with the platform separator
        // (`\` on Windows, exactly like Dart's `p.normalize`).
        let sep = MAIN_SEPARATOR;
        assert!(
            result.starts_with(&format!("{}{sep}", link_sub.to_string_lossy())),
            "expected symlink path preserved, got {result}"
        );
        assert!(!result.starts_with(&format!("{}{sep}", real_sub.to_string_lossy())));
    }

    #[test]
    fn io_error_display_no_path() {
        let err = IoError {
            message: "err".into(),
            path: None,
            kind: IoErrorKind::Other,
        };
        assert_eq!(err.to_string(), "err");
    }

    #[test]
    fn io_error_display_with_path() {
        let err = IoError {
            message: "err".into(),
            path: Some("/a".into()),
            kind: IoErrorKind::Other,
        };
        assert_eq!(err.to_string(), "err: /a");
    }

    #[test]
    fn io_error_is_not_found() {
        let not_found = IoError {
            message: "".into(),
            path: None,
            kind: IoErrorKind::NotFound,
        };
        assert!(not_found.is_not_found());

        let nil = IoError {
            message: "".into(),
            path: None,
            kind: IoErrorKind::Other,
        };
        assert!(!nil.is_not_found());
    }

    #[test]
    fn io_error_is_permission() {
        let perm = IoError {
            message: "".into(),
            path: None,
            kind: IoErrorKind::Permission,
        };
        assert!(perm.is_permission());

        let nil = IoError {
            message: "".into(),
            path: None,
            kind: IoErrorKind::Other,
        };
        assert!(!nil.is_permission());
    }

    #[test]
    fn io_error_is_already_exists() {
        let exists = IoError {
            message: "".into(),
            path: None,
            kind: IoErrorKind::AlreadyExists,
        };
        assert!(exists.is_already_exists());

        let nil = IoError {
            message: "".into(),
            path: None,
            kind: IoErrorKind::Other,
        };
        assert!(!nil.is_already_exists());
    }
}
