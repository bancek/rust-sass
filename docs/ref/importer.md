# Modules: `eval/import_cache.rs`, `eval/importer/`

Stylesheet resolution: turning `@use`/`@forward`/`@import` URLs into loaded
stylesheets. Ports `lib/src/import_cache.dart` (+ `async_import_cache.dart`,
logic-identical), `lib/src/importer.dart` (+ `importer/async.dart`), and
`lib/src/importer/*.dart`.

## `ImportCache`

```rust
pub struct ImportCache<'compile, 'parse> { /* per-importer canonicalize caches, load caches, load times, humanize data */ }
impl ImportCache {
    pub fn new_with_options(arena, importers, load_paths, sass_path, ...) -> Self;
    // Ordering: user importers → load-paths → SASS_PATH → package config (matches Dart).
    pub async fn canonicalize(&mut self, url, base_importer, base_url, for_import, warn_logger) -> SassResult<Option<CanonicalizeResult<'parse>>>;
    pub fn humanize(&self, canonical_url: &SassUrl) -> String;   // shortest-original-URL for stack frames
    pub fn source_map_url(&self, canonical_url: &SassUrl) -> SassUrl;
    pub fn load_time(&self, canonical_url: &SassUrl) -> Option<&SassTime>;
    pub fn clear_canonicalize(&mut self, canonical_url: &SassUrl);
    pub fn clear_import(&mut self, canonical_url: &SassUrl);
}
```

Ownership, not borrowing: created in `compile_string`, passed by value into
`evaluate()`, stored in `EvalState`, taken back via `state.import_cache.take()`
on success (see `ref/eval.md`, `ref/compile.md`). Relative (scheme-less) URLs
try the base importer first with the URL resolved against the base URL —
file bases via path-based `resolve_file_path` (preserves `..`), other bases
via `SassUrl::resolve`.

Exception: a `User` importer opting in via `prefers_raw_load_paths()`
(default `false`) is consulted on the base-importer fast path with the raw
load path and a forced containing URL (libsass `call_loader` shape, seeded
by `rust-sass-libsass`). Every other host sees the resolved URL and the
Dart containing rule, unchanged; `original_url` is whatever the importer
actually saw. Since the post-gate fix batch the same opt-in set is
additionally pre-consulted before the base importer for relative `@import`
URLs (`for_import` only, mirroring upstream's customs-first order); `None`
falls through to the order above, and pre-consult hits bypass the global
cache (upstream has none).

## `ImporterKind` + `Importer`

```rust
pub enum ImporterKind { Filesystem(FilesystemImporter), NoOp, User(Rc<dyn UserImporter>), Package(PackageImporter), NodePackage(NodePackageImporter) }
pub struct Importer<'parse>(&'parse ImporterKind);   // Copy arena handle
```

Only the `User` arm boxes (`Rc<dyn UserImporter>` — the JS/wasm host seam);
built-ins are inline. Files:

| File                               | Dart counterpart                            | Notes                                                                                                                                                                                                                                                                                                        |
| ---------------------------------- | ------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `importer/mod.rs`                  | `importer.dart` + `importer/async.dart`     | `could_canonicalize` defaults (`NoOp → false`); `is_non_canonical_scheme` (`pkg → true`); `Importer::canonicalize` is a convenience entry for non-eval callers that keeps `NoOpWarnLogger` — the eval pipeline threads the caller's logger via `ImportCache::canonicalize_with`, so do not "fix" the wrapper |
| `importer/filesystem.rs`           | `importer/filesystem.dart`                  | wrapped-relative-only load-path fallback (genuine absolute `file:` URLs resolve directly); relative-URL CWD deprecation warn                                                                                                                                                                                 |
| `importer/package.rs`              | `importer/package.dart`                     | `package:` scheme only (`None` for relative URLs too); "Unknown package." / "Unsupported URL"                                                                                                                                                                                                                |
| `importer/node_package.rs`         | `importer/node_package.dart`                | `pkg:` scheme (`None` for relative URLs); exports-map resolution in map insertion order; throws on non-relative targets, invalid exports values; `replaceFirst` for `*` patterns; missing package.json surfaces as a read error                                                                              |
| `importer/no_op.rs`                | `importer/no_op.dart`                       | all-`None`, `"(unknown)"` display                                                                                                                                                                                                                                                                            |
| `importer/result.rs`               | `importer/result.dart`                      | `{contents, syntax, source_map_url}`; missing `sourceMapUrl` falls back to a `data:` URL                                                                                                                                                                                                                     |
| `importer/resolve_import_path.rs`  | `importer/utils.dart` (`resolveImportPath`) | `.import`-first, sass→scss→css, partial-before-full, `index`                                                                                                                                                                                                                                                 |
| `importer/utils.rs`                | `importer/utils.dart`                       | `isValidUrlScheme`, `Syntax.forPath`                                                                                                                                                                                                                                                                         |
| `importer/canonicalize_context.rs` | `importer/canonicalize_context.dart`        | explicit `CanonicalizeContext{from_import, containing_url}` replacing Dart's Zone-ambient context; `containing_url()` access is tracked (`was_accessed`)                                                                                                                                                     |

## Error discipline

Importer `throw`s in Dart are plain strings (not `SassException`); the Rust
port uses `SassError::Script` (no span), which the load site wraps with span +
trace. Do not "upgrade" these to spanned errors at the throw site — the span
must come from the `@use`/`@import` rule, not the importer internals.

## Working here

- New importer kinds add an `ImporterKind` arm + a file under
  `eval/importer/`; the closed enum means the compiler lists every match
  site for you.
- Resolution-order changes are high-blast-radius (they shadow later
  importers): verify with the `directives/use`, `directives/import`, and
  `directives/forward` spec groups plus a differential CLI run.
- `modificationTime` has no Rust counterpart anywhere (Dart
  `filesystem.dart:101`, `importer.dart:40`); only dead-code stubs exist on
  `NoOp`/`NodePackage`. This is a watch-mode/staleness concern only —
  accepted gap, do not port it as part of behavior work.
