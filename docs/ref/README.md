# Module reference

Index of the per-module reference documents.

| Document                                         | Module(s)                                                                                                      |
| ------------------------------------------------ | -------------------------------------------------------------------------------------------------------------- |
| [`common.md`](common.md)                         | `common/` — errors, spans, source locations, the scanner                                                       |
| [`value.md`](value.md)                           | `value/` — the Sass value type and its concrete types                                                          |
| [`callable.md`](callable.md)                     | `callable.rs` — the function/mixin invocation machinery                                                        |
| [`functions.md`](functions.md)                   | `functions/` — the built-in Sass function library                                                              |
| [`ast.md`](ast.md)                               | `ast/` — the Sass and CSS abstract syntax trees                                                                |
| [`selector.md`](selector.md)                     | `selector/` — selector types and extend algorithms                                                             |
| [`parse.md`](parse.md)                           | `parse/` — the parser                                                                                          |
| [`extend.md`](extend.md)                         | `extend/` — the `@extend` store                                                                                |
| [`serialize.md`](serialize.md)                   | `serialize/` — the serializer                                                                                  |
| [`visitors.md`](visitors.md)                     | the visitor traits and `accept()` dispatch                                                                     |
| [`eval.md`](eval.md)                             | `eval/` — the evaluator                                                                                        |
| [`environment.md`](environment.md)               | `environment/`, `module/`, `configuration/` (warning-span fields live on `EvalState`; no `EvalContext` struct) |
| [`macros.md`](macros.md)                         | `rust-sass-macros` — the sync/async dual build                                                                 |
| [`math.md`](math.md)                             | `math.rs` — floating-point wrappers and `glibc-math`                                                            |
| [`io.md`](io.md)                                 | `io/` — the host I/O abstraction                                                                               |
| [`logger.md`](logger.md)                         | `logger/`, `deprecation.rs`                                                                                    |
| [`source-maps.md`](source-maps.md)               | `sourcemap/`, `source_map_buffer.rs`                                                                           |
| [`warn-logger.md`](warn-logger.md)               | deferred deprecation warnings                                                                                  |
| [`compile.md`](compile.md)                       | `compile/` — the public compile API                                                                            |
| [`sass-spec.md`](sass-spec.md)                   | the `sass-spec` suite: layout, `.hrx` format, debugging failures                                               |
| [`pipeline.md`](pipeline.md)                     | end-to-end walkthrough: arena ownership, entries, stages                                                       |
| [`compile-context.md`](compile-context.md)       | `compile_context.rs` — the per-compilation identity token                                                      |
| [`member-map.md`](member-map.md)                 | `member_map.rs` — ordered member views for `@forward` filtering                                                |
| [`url.md`](url.md)                               | `url.rs` — Dart-compatible URL resolution (`SassUrl`)                                                          |
| [`importer.md`](importer.md)                     | `eval/import_cache.rs`, `eval/importer/` — stylesheet resolution                                               |
| [`embedded.md`](embedded.md)                     | `rust-sass-embedded` — the protocol server                                                                     |
| [`sass-embedded-rust.md`](sass-embedded-rust.md) | `embedded-host-node-rust` — the published host API package                                                     |
| [`wasm.md`](wasm.md)                             | `rust-sass-wasm` — the JS bridge                                                                               |
| [`libsass.md`](libsass.md)                       | `rust-sass-libsass` — the libsass C-API adapter                                                                |
| [`util.md`](util.md)                             | `util/` — small shared helpers                                                                                 |

See [`../architecture.md`](../architecture.md) for how these fit together and
[`../README.md`](../README.md) for the project overview.
