# Module: `compile/`

The public compile API: `compile_string`, `compile`, and the CLI pipeline.

## Types

`CompileOptions<'compile, 'parse>` carries every compilation setting:
`importers`, `importer` (the entrypoint importer), `load_paths`, `sass_path`
(from `SASS_PATH`), `url`, `package_config`, `node_package_importer`,
`functions`, `logger`, `quiet_deps`, `source_map`, `include_source_map_sources`,
`emit_error_css`, `style`, `use_spaces` (default `true`), `indent_width`
(`Option<u32>`, default `None` = serializer default 2, mirroring Dart's
`indentWidth ??= 2`), `line_feed` (default LF),
`charset`, the `silence`/`fatal`/`future` deprecation lists, `verbose`,
`unicode`, `alert_color`, `alert_ascii`, and `syntax`. `CompileOptions::new(arena)`
constructs the defaults (the default `Importer` is a `NoOp` placeholder,
replaced by the compile entry-point importer at compile time).

```rust
impl CompileResult<'compile, 'parse> {
    pub fn css(&self) -> &str;
    pub fn source_map(&self) -> Option<&SingleMapping>;
    pub fn loaded_urls(&self) -> &[SassUrl];
}
```

## The compile flow

```
compile_string(source, io, opts)
├── parse source → Stylesheet<'parse>   (syntax: scss/sass/css)
├── build ImportCache from importers + load_paths
├── default importer → FilesystemImporter (no load path); logger → new_default_logger
├── wrap logger → DeprecationProcessingLogger (validate + summarize)
├── deprecation: COMPILE_STRING_RELATIVE_URL if relative URL and no node-package importer
├── compileStylesheet:
│   ├── LEGACY_JS_API deprecation if node_package_importer is set
│   ├── evaluate() → EvaluateResult   (on error, emitErrorCss renders the error as CSS)
│   └── serialize_with_source_map() → CSS + SingleMapping
└── return CompileResult
```

- `compile(path, ...)` reads the file, resolves the absolute `file:` URL,
  defaults `load_paths` to the file's directory, and infers syntax from the
  extension (`.sass` → Sass, `.css` → CSS, else SCSS).
- **Deprecation triggers:** `COMPILE_STRING_RELATIVE_URL` fires only when
  `span_url.scheme == ""` **and** `node_package_importer == None`; `LEGACY_JS_API`
  fires when `node_package_importer != None`.

## ImportCache ownership

The `ImportCache` flows by ownership, not borrowing: created in `compile_string`,
passed as an owned value into `evaluate()`, stored in `EvalState`, taken via
`state.import_cache.take()`, and returned on `EvaluateResult.import_cache`
(`Option`: `Some` on success, `None` if evaluation fails first). This avoids
lifetime entanglement between the cache and the evaluator.

## Error CSS

`exception_to_css_string(error)` renders a `SassError` as CSS:

1. Replace `*/` → `*∕` and `\r\n` → `\n`.
2. Escape non-ASCII as `\xHH ` for the `content:` property.
3. Wrap the message in a `/* ... */` comment.
4. Emit `body::before { ...; content: <escaped>; }`.

`emitErrorCss` applies only to the four spanned variants
(`Sass`/`Runtime`/`Format`/`MultiSpan`) — **never `Script`/`MultiSpanScript`**
(the unspanned `SassScriptException` family: I/O failures, `indentWidth`
validation, empty-overload internals). This mirrors Dart's `on SassException`
(`executable/compile_stylesheet.dart`): a parse error with `--error-css`
renders CSS and exits 65, while a missing input file exits 66.

## Source-map URL rewriting

After serialization, empty source-map URLs become `data:text/plain;charset=utf-8,`

- the escaped file text; non-empty URLs go through `import_cache.source_map_url()`.

## CLI

The CLI (`rust-sass-cli`) resolves `input.scss [output.css]` or
`input.scss:output.css`, handles stdin/stdout and directory compilation, and
exits `65` on a Sass error or `66` on a filesystem error (`StylesheetError`).
See the crate README for the flag reference.

## File mapping

| Dart                                        | Go                                                 | Rust                                                                                 |
| ------------------------------------------- | -------------------------------------------------- | ------------------------------------------------------------------------------------ |
| `lib/src/compile.dart`, `executable/*.dart` | `go-sass/compile/*.go`, `go-sass/cmd/go-sass/` CLI | `src/compile/{mod,options,result}.rs`, `rust-sass-cli/src/{main,options,compile}.rs` |
