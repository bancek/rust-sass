# Module: `io/`

The host I/O abstraction: the seam through which the compiler reads files,
canonicalizes paths, and reports filesystem state. It is the extension point a
JS/wasm host implements.

## The `Io` trait

```rust
#[maybe_async]
pub trait Io: fmt::Debug {
    fn read_file(&self, path) -> ...Result<Vec<u8>, IoError>;   // async
    fn file_exists(&self, path) -> ...Result<bool, IoError>;
    fn dir_exists(&self, path) -> ...Result<bool, IoError>;
    fn link_exists(&self, path) -> ...Result<bool, IoError>;
    fn canonicalize(&self, path) -> ...Result<String, IoError>;
    // ── sync (never async) ──
    fn current_dir(&self) -> String;   // falls back to "/"
    fn is_macos(&self) -> bool;
    fn is_windows(&self) -> bool;
    fn supports_ansi_escapes(&self) -> bool;
}
```

The compile-path methods are async (`LocalBoxFuture`, object-safe because the
trait is used as `Rc<dyn Io>`); the CLI/spec-only operations (`write_file`,
`delete_file`, `ensure_dir`, `list_dir`, `stat`, `read_stdin`, `exit_code`,
`print_output`, …) live on `IoExt: Io`. `DefaultIo` and `VirtualIo` implement
both; the wasm `JsIo` implements only `Io`.

**`current_dir()` stays sync** deliberately: it is the only `Io` call in the
entire error-formatting/logger path (`pretty_uri` → highlighter →
`to_error_string` → `StderrLogger` → `warn_fn` → stack traces). Making it async
would force all of that async. It always returns a value.

## `IoError`

```rust
pub struct IoError { message: String, kind: IoErrorKind, path: Option<String> }
pub enum IoErrorKind { NotFound, Permission, AlreadyExists, Other }
```

No `std::io` — wasm-safe. `is_not_found()`/`is_permission()`/`is_already_exists()`
map the kind to a predicate.

## Implementations

- **`DefaultIo`** — real filesystem. `canonicalize` is **lexical** (not
  `fs::canonicalize`): `clean_path(cwd.join(path))` plus a `real_case_path`
  case-corrector on case-insensitive filesystems, **preserving symlink names**
  (matching Dart).
- **`VirtualIo`** — in-memory filesystem for tests and the spec runner, with
  `with_files`/`add_file`/`set_current_dir`/`set_fallback` (a fallback `Rc<dyn
Io>` for unknown paths).

## Threading principles

Four rules govern how I/O flows through the code:

1. Use `Io::current_dir()` — never `std::env::current_dir()`.
2. Build `file:` URLs via `SassUrl::file_url_from_abs_path` — never string formatting.
3. Thread `Io` through `EvalConfig` (not per-function) — it is the central
   config; errors are constructed in eval but formatted in compile.
4. `Frame`/`Trace` do **not** store `Io` — `Frame::location(io)` and
   `Trace::format(io)` receive it from the caller (`impl Display for Frame` was
   removed accordingly).

Two further invariants:

- **Absolute-path invariant:** every path passed to
  `SassUrl::file_url_from_abs_path`
  must already be absolute (non-absolute paths are rejected);
  all call sites route through `io.canonicalize()` or an absolute CWD-joined
  path first.
- **`parse/import_url.rs` is off-limits:** it parses SCSS _source_ URL strings
  (Dart-compatible `Uri.parse` handling that returns the URL unchanged on
  success), not filesystem paths, so `file_url_from_abs_path` is explicitly _not_
  appropriate there.

## `SassUrl`

`SassUrl` (in `url.rs`) wraps `url::Url` with Sass semantics: `parse`,
`file_url_from_abs_path`, `scheme`, `host`, `query`, `fragment`, `join`,
`resolve`, `is_file`, `is_relative`, and `Display` (which strips the internal
`sass-relative:` prefix — always use `Display`, never `as_str()`, when passing
URLs to importers).

## File mapping

| Dart                                     | Go                                                                     | Rust                                    |
| ---------------------------------------- | ---------------------------------------------------------------------- | --------------------------------------- |
| `lib/src/io.dart`                        | `go/sassio/*.go`                                                       | `src/io/{mod,default_io,virtual_io}.rs` |
| `(external) package:path` (canonicalize) | `go/sassio/default_io_canonicalize.go`, `default_io_real_case_path.go` | `src/io/default_io.rs`                  |
| (not present in Dart/Go)                 | `go/sassurl/sassurl.go`                                                | `src/url.rs`                            |
