// dart-source: N/A (Go-only, based on go/spec/virtual_io.go)
// go-source: go/spec/virtual_io.go

use crate::common::time::SassTime;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::path::MAIN_SEPARATOR;
use std::rc::Rc;

#[cfg(feature = "async")]
use futures::future::LocalBoxFuture;

use crate::io::{clean_path, Io, IoError, IoErrorKind, IoExt, IoMetadata};

/// A fully in-memory Io implementation for tests and virtual environments.
///
/// Provides a virtual filesystem with configurable current directory (defaults
/// to `"/"`). All operations are deterministic and do not touch the real
/// filesystem.  Use `set_fallback()` to delegate unknown paths to a real `Io`
/// implementation (e.g. for the spec runner).
pub struct VirtualIo {
    files: RefCell<HashMap<String, String>>,
    current_dir: RefCell<String>,
    output: RefCell<String>,
    exit_code: Cell<i32>,
    fallback: RefCell<Option<Rc<dyn IoExt>>>,
}

impl fmt::Debug for VirtualIo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VirtualIo")
            .field("files_count", &self.files.borrow().len())
            .field("current_dir", &self.current_dir.borrow())
            .finish()
    }
}

impl Default for VirtualIo {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtualIo {
    /// Creates an empty virtual filesystem with CWD set to `"/"`.
    pub fn new() -> Self {
        VirtualIo {
            files: RefCell::new(HashMap::new()),
            current_dir: RefCell::new("/".to_string()),
            output: RefCell::new(String::new()),
            exit_code: Cell::new(0),
            fallback: RefCell::new(None),
        }
    }

    /// Creates an empty virtual filesystem with an explicit current directory.
    pub fn with_cwd(cwd: &str) -> Self {
        VirtualIo {
            files: RefCell::new(HashMap::new()),
            current_dir: RefCell::new(cwd.to_string()),
            output: RefCell::new(String::new()),
            exit_code: Cell::new(0),
            fallback: RefCell::new(None),
        }
    }

    /// Creates a virtual filesystem pre-populated with the given files.
    /// CWD defaults to `"/"`. Keys are normalized exactly like every lookup
    /// (`add_file` cleans too), so `/a/../b` and `/b` name the same file on
    /// every platform.
    pub fn with_files(files: HashMap<String, String>) -> Self {
        let files = files
            .into_iter()
            .map(|(k, v)| (clean_path(&k), v))
            .collect();
        VirtualIo {
            files: RefCell::new(files),
            current_dir: RefCell::new("/".to_string()),
            output: RefCell::new(String::new()),
            exit_code: Cell::new(0),
            fallback: RefCell::new(None),
        }
    }

    /// Sets the current working directory.
    pub fn set_current_dir(&self, cwd: &str) {
        *self.current_dir.borrow_mut() = cwd.to_string();
    }

    /// Adds a file to the virtual filesystem.
    pub fn add_file(&self, path: &str, content: &str) {
        let key = clean_path(path);
        self.files.borrow_mut().insert(key, content.to_string());
    }

    /// Sets a fallback `IoExt` for paths not found in the virtual filesystem.
    pub fn set_fallback(&self, io: Rc<dyn IoExt>) {
        *self.fallback.borrow_mut() = Some(io);
    }

    /// Returns all output written via `print_output`.
    pub fn output_buffer(&self) -> String {
        self.output.borrow().clone()
    }
}

/// Directory-prefix form of a cleaned key for `starts_with` matching.
/// Uses the platform separator so virtual trees match on Windows exactly
/// like unix (`/` there — byte-identical).
fn dir_prefix_of(key: &str) -> String {
    let sep = MAIN_SEPARATOR;
    if key.ends_with(sep) {
        key.to_string()
    } else {
        format!("{key}{sep}")
    }
}

#[rust_sass_macros::maybe_async]
// `RefCell` borrows are held across `.await` in the fallback-delegation
// methods below. Sound by design: the compiler is single-threaded (`!Send`)
// and `Io` implementations never re-enter Rust mid-request, so a suspended
// future cannot observe a conflicting borrow (see critical-invariants.md
// "No-reentry contract"). Do not "fix" with defensive guard-dropping.
#[allow(clippy::await_holding_refcell_ref)]
impl Io for VirtualIo {
    fn current_dir(&self) -> String {
        self.current_dir.borrow().clone()
    }

    fn read_file<'a>(&'a self, path: &'a Path) -> LocalBoxFuture<'a, Result<Vec<u8>, IoError>> {
        Box::pin(async move {
            let key = clean_path(&path.to_string_lossy());
            let files = self.files.borrow();
            if let Some(content) = files.get(&key) {
                return Ok(content.as_bytes().to_vec());
            }
            drop(files);
            if let Some(ref fb) = *self.fallback.borrow() {
                fb.read_file(path).await
            } else {
                Err(IoError {
                    message: "file not found".to_string(),
                    path: Some(key),
                    kind: IoErrorKind::NotFound,
                })
            }
        })
    }

    fn file_exists<'a>(&'a self, path: &'a Path) -> LocalBoxFuture<'a, Result<bool, IoError>> {
        Box::pin(async move {
            let key = clean_path(&path.to_string_lossy());
            if self.files.borrow().contains_key(&key) {
                return Ok(true);
            }
            if let Some(ref fb) = *self.fallback.borrow() {
                fb.file_exists(path).await
            } else {
                Ok(false)
            }
        })
    }

    fn dir_exists<'a>(&'a self, path: &'a Path) -> LocalBoxFuture<'a, Result<bool, IoError>> {
        Box::pin(async move {
            let key = clean_path(&path.to_string_lossy());
            let dir_prefix = dir_prefix_of(&key);
            {
                let files = self.files.borrow();
                for p in files.keys() {
                    if clean_path(p).starts_with(&dir_prefix) {
                        return Ok(true);
                    }
                }
            }
            if let Some(ref fb) = *self.fallback.borrow() {
                fb.dir_exists(Path::new(&dir_prefix)).await
            } else {
                Ok(false)
            }
        })
    }

    fn link_exists<'a>(&'a self, _path: &'a Path) -> LocalBoxFuture<'a, Result<bool, IoError>> {
        Box::pin(async move { Ok(false) })
    }

    fn canonicalize<'a>(&'a self, path: &'a Path) -> LocalBoxFuture<'a, Result<String, IoError>> {
        Box::pin(async move { Ok(clean_path(&path.to_string_lossy())) })
    }

    fn is_windows(&self) -> bool {
        false
    }

    fn is_macos(&self) -> bool {
        false
    }

    fn supports_ansi_escapes(&self) -> bool {
        false
    }
}

#[rust_sass_macros::maybe_async]
// Same `await_holding_refcell_ref` rationale as the `Io` impl above:
// fallback delegation holds a `RefCell` borrow across `.await`, sound under
// the no-reentry contract (critical-invariants.md).
#[allow(clippy::await_holding_refcell_ref)]
impl IoExt for VirtualIo {
    fn write_file<'a>(
        &'a self,
        path: &'a Path,
        contents: &'a [u8],
    ) -> LocalBoxFuture<'a, Result<(), IoError>> {
        Box::pin(async move {
            let key = clean_path(&path.to_string_lossy());
            let content = String::from_utf8_lossy(contents).to_string();
            self.files.borrow_mut().insert(key, content);
            Ok(())
        })
    }

    fn delete_file<'a>(&'a self, path: &'a Path) -> LocalBoxFuture<'a, Result<(), IoError>> {
        Box::pin(async move {
            let key = clean_path(&path.to_string_lossy());
            self.files.borrow_mut().remove(&key);
            Ok(())
        })
    }

    fn read_stdin(&self) -> LocalBoxFuture<'_, Result<Vec<u8>, IoError>> {
        Box::pin(async move { Ok(Vec::new()) })
    }

    fn ensure_dir<'a>(&'a self, _path: &'a Path) -> LocalBoxFuture<'a, Result<(), IoError>> {
        Box::pin(async move { Ok(()) })
    }

    fn list_dir<'a>(
        &'a self,
        path: &'a Path,
        recursive: bool,
    ) -> LocalBoxFuture<'a, Result<Vec<String>, IoError>> {
        Box::pin(async move {
            let key = clean_path(&path.to_string_lossy());
            let dir_prefix = dir_prefix_of(&key);
            let mut result = Vec::new();
            let mut seen = HashMap::new();
            {
                let files = self.files.borrow();
                for p in files.keys() {
                    let cleaned = clean_path(p);
                    if !cleaned.starts_with(&dir_prefix) {
                        continue;
                    }
                    let rel = &cleaned[dir_prefix.len()..];
                    if !recursive && rel.contains(MAIN_SEPARATOR) {
                        continue;
                    }
                    if !seen.contains_key(&cleaned) {
                        result.push(cleaned.clone());
                        seen.insert(cleaned.clone(), true);
                    }
                }
            }
            Ok(result)
        })
    }

    fn get_environment_variable(&self, _name: &str) -> Option<String> {
        None
    }

    fn exit_code(&self) -> i32 {
        self.exit_code.get()
    }

    fn set_exit_code(&self, code: i32) {
        self.exit_code.set(code);
    }

    fn print_output(&self, message: &str) {
        self.output.borrow_mut().push_str(message);
    }

    fn safe_print(&self, _message: &str) {}

    fn print_error(&self, _message: &str) {}

    fn has_terminal(&self) -> bool {
        false
    }

    fn stat<'a>(&'a self, path: &'a Path) -> LocalBoxFuture<'a, Result<IoMetadata, IoError>> {
        Box::pin(async move {
            let key = clean_path(&path.to_string_lossy());
            {
                let files = self.files.borrow();
                if let Some(_content) = files.get(&key) {
                    return Ok(IoMetadata::new(SassTime::UNIX_EPOCH));
                }
                let dir_prefix = dir_prefix_of(&key);
                for p in files.keys() {
                    if clean_path(p).starts_with(&dir_prefix) {
                        return Ok(IoMetadata::new(SassTime::UNIX_EPOCH));
                    }
                }
            }
            if let Some(ref fb) = *self.fallback.borrow() {
                fb.stat(path).await
            } else {
                Err(IoError {
                    message: "no such file or directory".to_string(),
                    path: Some(key),
                    kind: IoErrorKind::NotFound,
                })
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[rust_sass_macros::maybe_test]
    async fn test_default_cwd_is_root() {
        let io = VirtualIo::new();
        assert_eq!(io.current_dir(), "/");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_with_cwd() {
        let io = VirtualIo::with_cwd("/home/user");
        assert_eq!(io.current_dir(), "/home/user");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_set_current_dir() {
        let io = VirtualIo::new();
        io.set_current_dir("/tmp");
        assert_eq!(io.current_dir(), "/tmp");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_with_files_defaults_cwd_to_root() {
        let mut files = HashMap::new();
        files.insert("/a.scss".to_string(), "x".to_string());
        let io = VirtualIo::with_files(files);
        assert_eq!(io.current_dir(), "/");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_read_file_found() {
        let mut files = HashMap::new();
        files.insert("/abs/test.txt".to_string(), "hello".to_string());
        let io = VirtualIo::with_files(files);
        let result = io.read_file(Path::new("/abs/test.txt")).await.unwrap();
        assert_eq!(result, b"hello");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_read_file_not_found() {
        let io = VirtualIo::new();
        let err = io.read_file(Path::new("/nonexistent")).await.unwrap_err();
        assert_eq!(err.path, Some(clean_path("/nonexistent")));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_file_exists() {
        let mut files = HashMap::new();
        files.insert("/abs/test.txt".to_string(), "hello".to_string());
        let io = VirtualIo::with_files(files);
        let __awaited18 = io.file_exists(Path::new("/abs/test.txt")).await.unwrap();

        assert!(__awaited18);
        let __awaited19 = !io.file_exists(Path::new("/abs/other.txt")).await.unwrap();

        assert!(__awaited19);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_dir_exists() {
        let mut files = HashMap::new();
        files.insert("/abs/sub/file.txt".to_string(), "content".to_string());
        let io = VirtualIo::with_files(files);
        let __awaited20 = io.dir_exists(Path::new("/abs/sub")).await.unwrap();

        assert!(__awaited20);
        let __awaited21 = io.dir_exists(Path::new("/abs")).await.unwrap();

        assert!(__awaited21);
        let __awaited22 = !io.dir_exists(Path::new("/other")).await.unwrap();

        assert!(__awaited22);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_write_file() {
        let io = VirtualIo::new();
        io.write_file(Path::new("/new/file.txt"), b"data")
            .await
            .unwrap();
        let __awaited23 = io.file_exists(Path::new("/new/file.txt")).await.unwrap();

        assert!(__awaited23);
        let result = io.read_file(Path::new("/new/file.txt")).await.unwrap();
        assert_eq!(result, b"data");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_delete_file() {
        let mut files = HashMap::new();
        files.insert("/abs/test.txt".to_string(), "hello".to_string());
        let io = VirtualIo::with_files(files);
        let __awaited24 = io.file_exists(Path::new("/abs/test.txt")).await.unwrap();

        assert!(__awaited24);
        io.delete_file(Path::new("/abs/test.txt")).await.unwrap();
        let __awaited25 = !io.file_exists(Path::new("/abs/test.txt")).await.unwrap();

        assert!(__awaited25);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_canonicalize() {
        let io = VirtualIo::new();
        let result = io.canonicalize(Path::new("/a/b/../c")).await.unwrap();
        assert_eq!(result, clean_path("/a/c"));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_print_output() {
        let io = VirtualIo::new();
        io.print_output("hello");
        io.print_output(" world");
        assert_eq!(io.output_buffer(), "hello world");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_stat_file() {
        let mut files = HashMap::new();
        files.insert("/abs/test.txt".to_string(), "content".to_string());
        let io = VirtualIo::with_files(files);
        let meta = io.stat(Path::new("/abs/test.txt")).await.unwrap();
        assert!(meta.modified().is_ok());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_stat_not_found() {
        let io = VirtualIo::new();
        let err = io.stat(Path::new("/nonexistent")).await.unwrap_err();
        assert_eq!(err.path, Some(clean_path("/nonexistent")));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_exit_code() {
        let io = VirtualIo::new();
        assert_eq!(io.exit_code(), 0);
        io.set_exit_code(42);
        assert_eq!(io.exit_code(), 42);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_list_dir_non_recursive() {
        let mut files = HashMap::new();
        files.insert("/abs/a.txt".to_string(), "a".to_string());
        files.insert("/abs/sub/b.txt".to_string(), "b".to_string());
        let io = VirtualIo::with_files(files);
        let result = io.list_dir(Path::new("/abs"), false).await.unwrap();
        assert_eq!(result.len(), 1);
        assert!(result.contains(&clean_path("/abs/a.txt")));
    }

    #[rust_sass_macros::maybe_test]
    async fn test_list_dir_recursive() {
        let mut files = HashMap::new();
        files.insert("/abs/a.txt".to_string(), "a".to_string());
        files.insert("/abs/sub/b.txt".to_string(), "b".to_string());
        let io = VirtualIo::with_files(files);
        let result = io.list_dir(Path::new("/abs"), true).await.unwrap();
        assert_eq!(result.len(), 2);
    }

    #[rust_sass_macros::maybe_test]
    async fn test_add_file() {
        let io = VirtualIo::new();
        io.add_file("/x/style.scss", "body { color: red; }");
        let __awaited26 = io.file_exists(Path::new("/x/style.scss")).await.unwrap();

        assert!(__awaited26);
    }
}
