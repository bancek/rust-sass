# The compile pipeline, end to end

One page showing how a Sass string becomes CSS: who owns what, which function
runs at each stage, and where the arena lifetimes unify. Read this before
tracing any compile through the code. For stage internals, follow the links
to the per-module refs.

## Ownership first

The caller owns everything expensive:

```rust
let arena = Bump::new();                                        // 1. the arena
let io: Rc<dyn Io> = Rc::new(DefaultIo::new());                 // 2. the host
let options = CompileOptions::new(&arena);                      // 3. the settings
let result = compile_string("a { b: 1; }", io, options, &arena)?;
```

- Source text, AST nodes, spans, Sass values, and every temporary string live
  in the `Bump` for the duration of the call. Nothing is freed until the
  caller drops the arena — so a `&'parse str` may point at a transient eval
  buffer without outliving it.
- `Io` (`Rc<dyn Io>`) is the host seam (filesystem, CWD, case behavior).
- `CompileResult` borrows from the arena (`CompileResult<'compile, 'parse>`);
  at the concrete call site both lifetimes unify to the arena borrow, which
  is why every public entry point carries `where 'compile: 'parse,
'parse: 'compile`.

## The three entries

| Entry                                     | Input       | Extra work before `compile_string`                                                                                                                   |
| ----------------------------------------- | ----------- | ---------------------------------------------------------------------------------------------------------------------------------------------------- |
| `compile_string(source, io, opts, arena)` | source text | none — the pipeline itself                                                                                                                           |
| `compile(path, io, opts, arena)`          | file path   | `read_file` → `canonicalize` → `file:` URL → UTF-8 decode (`Invalid UTF-8.` on bad bytes) → syntax from extension (explicit `Scss` also re-inferred) |
| `compile_string_to_result(…)`             | source text | convenience shim with default options                                                                                                                |

## `compile_string`, in order (`compile/mod.rs`)

1. **Parse** — `Stylesheet::parse_scss/sass/css(source, url, …, arena)` by
   `opts.syntax`. Parse errors are `ParseError`s converted to `SassError`
   here. See `ref/parse.md`.
2. **Import cache** — `ImportCache::new_with_options(arena, importers,
load_paths, SASS_PATH-env-or-opt, …, io, package_config)`. Ordering:
   user importers → load-paths → `SASS_PATH` → package config.
   See `ref/importer.md`.
3. **Logger wrap** — user logger (or `new_default_logger`) is always wrapped
   in `DeprecationProcessingLogger` (silence/fatal/future lists, repetition
   limit); `dpl.validate()` runs **before** evaluation. See `ref/logger.md`.
4. **Deprecations** — `COMPILE_STRING_RELATIVE_URL` if `url` is relative with
   no node-package importer.
5. **Default importer** — a `NoOp` placeholder `opts.importer` becomes
   `FilesystemImporter::new_no_load_path`.
6. **`compile_stylesheet_inner(...)`** — evaluate + serialize (below).
7. **`dpl.summarize(js)`** — repetition summary on success _and_ failure.

## `compile_stylesheet_inner` (`compile/mod.rs`)

1. **LEGACY_JS_API** deprecation if a node-package importer is set.
2. **`eval::evaluate(stylesheet, Some(import_cache), node_importer, importer,
functions, logger, quiet_deps, source_map, unicode, alert_color,
alert_ascii, arena, io)`** — see below. Returns `EvaluateResult
{ stylesheet: CssStylesheet, loaded_urls, import_cache }`.
3. **Serialize** — `serialize::serialize(&stylesheet, &SerializeOptions {
style, inspect: false, use_spaces: true, indent_width: 2, line_feed: LF,
charset, source_map, include_source_map_sources })`. See `ref/serialize.md`
   - `ref/source-maps.md`.
4. **Source-map URL rewrite** — empty URLs become `data:` URLs from the entry
   source text; others resolve via `import_cache.source_map_url()`.
5. **`CompileResult::new(eval_result, ser_result)`**.
6. **Error path** — if `emit_error_css` and `is_sass_exception(err)`
   (the four spanned variants `Sass`/`Runtime`/`Format`/`MultiSpan` — never
   the unspanned `Script`/`MultiSpanScript`, mirroring Dart's
   `on SassException`), the error renders
   as CSS (`exception_to_css_string`) inside a synthetic `Ok`; otherwise the
   error propagates. See `ref/compile.md`.

## `evaluate()` (`eval/mod.rs`)

1. `EvaluateVisitor::new(logger, new_compile_context(), arena, io)` — fresh
   `{config, state, arena}` bundle plus a fresh `CompileContext` identity
   token (see `ref/compile-context.md`).
2. Flags in (`unicode`, `alert_color`, `alert_ascii`, `quiet_deps`,
   `source_map`, `node_importer`), import cache installed,
   **user functions first** (globals overwrite), then built-ins registered
   (`eval/init.rs` + `eval/meta.rs`).
3. `evaluate_stylesheet` runs inside `with_evaluation_context` (stylesheet
   span = deprecation fallback) — free functions
   `evaluate_statement/expression/css_* (config, state, arena, …)` + `match`,
   **no visitor-trait impls**. See `ref/eval.md`.
4. Module assembly: `env.to_module(...)` → `combine_css(...)` merges
   `@use`d CSS, freezes the modifiable tree (`to_css_node`) to the returned
   `CssStylesheet`; `loaded_urls` collected; `import_cache` handed back on
   the result for step 4 above.

## What dies when

- The evaluator (`EvaluateVisitor`, `EvalState`) drops at the end of
  `evaluate()`; the arena (and everything in it — AST, values, CSS,
  `CompileResult`) lives until the caller drops the `Bump`.
- `Rc` handles (`Logger`, `Io`, `CompileContext`, CSS parent links,
  callable overloads) outlive individual borrows by design; everything else
  is a plain `&'parse` borrow. See `architecture.md` §5.

## Tracing a compile (where to look)

| Question                             | Start here                                                                                        |
| ------------------------------------ | ------------------------------------------------------------------------------------------------- |
| What did the parser produce?         | `Stylesheet::parse_scss` return in `compile_string`; `parse/` per `ref/parse.md`                  |
| Which importer resolved a URL?       | `ImportCache::canonicalize` (`eval/import_cache.rs`); kinds in `ref/importer.md`                  |
| Why is a variable/function missing?  | `Environment` lookups (`environment/mod.rs`); views in `ref/environment.md` + `ref/member-map.md` |
| What span/trace will an error carry? | `exception()` + `add_exception_span` (`eval/helpers.rs`); `ref/eval.md` error flow                |
| What CSS came out?                   | `EvaluateResult.stylesheet` (frozen `CssNode`); serialization in `ref/serialize.md`               |
| What will the source map contain?    | `ser_result.source_map` + rewrite step 4; `ref/source-maps.md`                                    |

## File mapping

| Stage            | Dart                                         | Rust                                         |
| ---------------- | -------------------------------------------- | -------------------------------------------- |
| entry + pipeline | `lib/src/compile.dart`                       | `compile/{mod,options,result}.rs`            |
| executable layer | `lib/src/executable/compile_stylesheet.dart` | `compile/mod.rs:415+` (`compile_stylesheet`) |
| evaluate         | `lib/src/visitor/evaluate.dart`              | `eval/*.rs`                                  |
| serialize        | `lib/src/visitor/serialize.dart`             | `serialize/*.rs`                             |
