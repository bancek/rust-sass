// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/io/interface.dart + lib/src/io/vm.dart (interface only)
// go-source: go/sassio/io_interface.go + go/sassio/errors.go

pub mod default_io;
pub mod virtual_io;

use crate::common::time::SassTime;
use std::collections::HashMap;
use std::fmt;
use std::fs::Metadata;
use std::io::Error;
use std::path::Component;
use std::path::MAIN_SEPARATOR;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[cfg(feature = "async")]
use futures::future::LocalBoxFuture;

pub use default_io::DefaultIo;
pub use virtual_io::VirtualIo;

/// Normalizes a path by resolving `.` and `..` components.
/// Does NOT access the real filesystem.
/// Lexically normalizes `p` (resolves `.`/`..`, drops redundant separators)
/// without touching the filesystem — symlinks are preserved, unlike
/// [`std::fs::canonicalize`]. Matches Dart's `p.normalize`.
///
/// The root is preserved exactly: `/` on unix, the drive prefix plus root on
/// Windows (`C:\…`, `\\server\share\…`). Components join with the platform
/// separator, so Windows results use `\` exactly like Dart's.
pub fn clean_path(p: &str) -> String {
    let path = Path::new(p);
    let mut prefix = String::new();
    let mut has_root = false;
    let mut components: Vec<&str> = Vec::new();
    for c in path.components() {
        match c {
            Component::Prefix(pref) => {
                prefix = pref.as_os_str().to_string_lossy().into_owned();
            }
            Component::RootDir => has_root = true,
            Component::CurDir => {}
            Component::ParentDir => {
                components.pop();
            }
            Component::Normal(s) => {
                if let Some(s) = s.to_str() {
                    components.push(s);
                }
            }
        }
    }
    let sep = MAIN_SEPARATOR;
    let mut result = String::new();
    result.push_str(&prefix);
    if has_root {
        result.push(sep);
    }
    for (i, c) in components.iter().enumerate() {
        if i > 0 {
            result.push(sep);
        }
        result.push_str(c);
    }
    if result.is_empty() && !p.is_empty() && p != "." {
        result = p.to_string();
    }
    result
}

fn eq_ignore_case(a: &str, b: &str) -> bool {
    a.chars()
        .flat_map(|c| c.to_lowercase())
        .eq(b.chars().flat_map(|c| c.to_lowercase()))
}

/// Corrects the casing of each path component to match the real filesystem,
/// using a case-insensitive read of each directory (supplied by [read_dir],
/// which returns the full paths of a directory's entries, or `None` on error).
/// Results are cached in [cache].
///
/// Matches Dart: `_realCasePath` in lib/src/io.dart. Because it matches
/// directory entries (which include symlink names verbatim), symlinks are
/// preserved rather than resolved — unlike `std::fs::canonicalize`.
///
/// This is the SINGLE Rust implementation, shared by `DefaultIo` (real fs) and
/// the wasm `JsIo` (delegate `readDir`). See docs/ref/wasm.md ("Io bridge").
pub fn real_case_path(
    path: &str,
    cache: &mut HashMap<String, String>,
    mut read_dir: impl FnMut(&str) -> Option<Vec<String>>,
) -> String {
    real_case_helper(path, cache, &mut read_dir)
}

fn real_case_helper(
    path: &str,
    cache: &mut HashMap<String, String>,
    read_dir: &mut impl FnMut(&str) -> Option<Vec<String>>,
) -> String {
    let p = Path::new(path);
    let Some(parent) = p.parent() else {
        return path.to_string();
    };
    if parent.as_os_str() == p.as_os_str() {
        return path.to_string();
    }
    if let Some(cached) = cache.get(path) {
        return cached.clone();
    }

    let real_dir = real_case_helper(&parent.to_string_lossy(), cache, read_dir);
    let base = p.file_name().unwrap().to_string_lossy().to_string();

    let result = match read_dir(&real_dir) {
        Some(entries) => {
            let mut found = None;
            for entry in entries {
                let name = match Path::new(&entry).file_name() {
                    Some(n) => n.to_string_lossy().to_string(),
                    None => entry.clone(),
                };
                if eq_ignore_case(&name, &base) {
                    let mut result = PathBuf::from(&real_dir);
                    result.push(&name);
                    found = Some(result.to_string_lossy().to_string());
                    break;
                }
            }
            found.unwrap_or_else(|| {
                let mut result = PathBuf::from(&real_dir);
                result.push(&base);
                result.to_string_lossy().to_string()
            })
        }
        None => {
            let mut result = PathBuf::from(&real_dir);
            result.push(&base);
            result.to_string_lossy().to_string()
        }
    };

    cache.insert(path.to_string(), result.clone());
    result
}

#[derive(Debug, Clone)]
pub struct IoMetadata {
    pub modified: SassTime,
}

impl IoMetadata {
    pub const fn new(modified: SassTime) -> Self {
        Self { modified }
    }

    pub fn modified(&self) -> Result<SassTime, Error> {
        Ok(self.modified)
    }
}

impl From<Metadata> for IoMetadata {
    fn from(m: Metadata) -> Self {
        let d = m.modified().unwrap_or(UNIX_EPOCH);
        Self {
            modified: SassTime::from_millis(
                d.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as i64,
            ),
        }
    }
}

/// The minimal `Io` surface needed by the library compile path (and the wasm
/// bridge). CLI/spec-only operations live on [`IoExt`]. The wasm `JsIo`
/// implements ONLY this trait. See docs/ref/wasm.md ("Io bridge").
///
/// `file_exists`/`dir_exists`/`link_exists` return `Result` because Dart
/// rethrows non-ENOENT errors (e.g. `EACCES`); a bare `bool` would swallow
/// them. Implementations map ENOENT → `Ok(false)` and real errors → `Err`.
#[rust_sass_macros::maybe_async]
pub trait Io: fmt::Debug {
    /// Reads the file at `path`.
    ///
    /// Returns the raw bytes; UTF-8 decoding is the caller's job. Throws an
    /// [`IoError`] if reading fails (Dart's `FileSystemException`).
    fn read_file<'a>(&'a self, path: &'a Path) -> LocalBoxFuture<'a, Result<Vec<u8>, IoError>>;
    /// Returns whether a file at `path` exists.
    fn file_exists<'a>(&'a self, path: &'a Path) -> LocalBoxFuture<'a, Result<bool, IoError>>;
    /// Returns whether a dir at `path` exists.
    fn dir_exists<'a>(&'a self, path: &'a Path) -> LocalBoxFuture<'a, Result<bool, IoError>>;
    /// Returns whether a symbolic link at `path` exists.
    fn link_exists<'a>(&'a self, path: &'a Path) -> LocalBoxFuture<'a, Result<bool, IoError>>;
    /// Returns the canonical form of `path` on disk.
    fn canonicalize<'a>(&'a self, path: &'a Path) -> LocalBoxFuture<'a, Result<String, IoError>>;
    /// Returns the current working directory. Always returns a value;
    /// implementations that cannot determine the CWD fall back to `"/"`.
    fn current_dir(&self) -> String;
    /// Whether the current process is running on Windows.
    fn is_windows(&self) -> bool;
    /// Whether the current process is running on Mac OS.
    fn is_macos(&self) -> bool;
    /// Whether this process is connected to a terminal that supports ANSI
    /// escape sequences.
    fn supports_ansi_escapes(&self) -> bool;
}

/// The CLI/spec-runner extension of [`Io`] (compile-to-file, stdin, watch,
/// process exit codes, output). The wasm bridge does NOT implement this; the
/// JS delegate has no corresponding methods.
#[rust_sass_macros::maybe_async]
pub trait IoExt: Io {
    /// Writes `contents` to the file at `path`.
    ///
    /// Throws an [`IoError`] if writing fails (Dart encodes as UTF-8 and
    /// throws `FileSystemException`).
    fn write_file<'a>(
        &'a self,
        path: &'a Path,
        contents: &'a [u8],
    ) -> LocalBoxFuture<'a, Result<(), IoError>>;
    /// Deletes the file at `path`.
    ///
    /// Throws an [`IoError`] if deletion fails.
    fn delete_file<'a>(&'a self, path: &'a Path) -> LocalBoxFuture<'a, Result<(), IoError>>;
    /// Ensures that a directory exists at `path`, creating it and its
    /// ancestors if necessary.
    fn ensure_dir<'a>(&'a self, path: &'a Path) -> LocalBoxFuture<'a, Result<(), IoError>>;
    /// Lists the files (not sub-directories) in the directory at `path`.
    ///
    /// If `recursive` is `true`, this lists files in directories transitively
    /// beneath `path` as well.
    fn list_dir<'a>(
        &'a self,
        path: &'a Path,
        recursive: bool,
    ) -> LocalBoxFuture<'a, Result<Vec<String>, IoError>>;
    /// Reads from the standard input for the current process until it closes,
    /// returning the contents.
    fn read_stdin(&self) -> LocalBoxFuture<'_, Result<Vec<u8>, IoError>>;
    /// Returns the modification time of the file at `path` (Dart's
    /// `modificationTime`, reshaped as `stat` returning [`IoMetadata`]).
    fn stat<'a>(&'a self, path: &'a Path) -> LocalBoxFuture<'a, Result<IoMetadata, IoError>>;
    /// Gets and sets the exit code that the process will use when it exits.
    fn exit_code(&self) -> i32;
    fn set_exit_code(&self, code: i32);
    fn print_output(&self, message: &str);
    /// Prints `message` (followed by a newline) to standard output or the
    /// equivalent.
    ///
    /// This method is thread-safe.
    fn safe_print(&self, message: &str);
    /// Prints `message` (followed by a newline) to standard error or the
    /// equivalent.
    ///
    /// This method is thread-safe.
    fn print_error(&self, message: &str);
    /// Returns the value of the environment variable with the given `name`,
    /// or `None` if it's not set.
    fn get_environment_variable(&self, name: &str) -> Option<String>;
    /// Returns whether or not stdout is connected to an interactive terminal.
    fn has_terminal(&self) -> bool;
}

/// An error thrown by [`Io::read_file`].
///
/// Mirrors Go's FileSystemException.
#[derive(Debug)]
pub struct IoError {
    pub message: String,
    pub kind: IoErrorKind,
    pub path: Option<String>,
}

/// The category of an I/O error, derived from `std::io::ErrorKind`. Needed so
/// `IoError` can be WASM-safe (no `std::io` dependency).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoErrorKind {
    NotFound,
    Permission,
    AlreadyExists,
    Other,
}

impl IoError {
    pub fn is_not_found(&self) -> bool {
        matches!(self.kind, IoErrorKind::NotFound)
    }

    pub fn is_permission(&self) -> bool {
        matches!(self.kind, IoErrorKind::Permission)
    }

    pub fn is_already_exists(&self) -> bool {
        matches!(self.kind, IoErrorKind::AlreadyExists)
    }
}

impl fmt::Display for IoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ref path) = self.path {
            write!(f, "{}: {}", self.message, path)
        } else {
            write!(f, "{}", self.message)
        }
    }
}
