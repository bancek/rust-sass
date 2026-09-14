# Modules: `sourcemap/`, `source_map_buffer.rs`

Source-map generation: the VLQ encoder, the V3 builder, and the buffer that
records mappings during serialization.

## VLQ

The V3 source-map variable-length quantity encoder:

- The sign is folded into the least-significant bit (`encode(2*value)` for
  non-negative, `encode(-2*value - 1)` for negative).
- Values are emitted as 5-bit chunks with a continuation bit.
- Each chunk is mapped through the Base64 alphabet.

## SourceMapBuilder

Accumulates `(generated_line, generated_column, source_line, source_column)`
entries and emits a V3 `SourceMapJson` (serde). Two behaviors match Dart exactly:

- **`_addEntry` dedup** — an entry is skipped when the last entry shares the same
  source line and target line (browsers don't care about position within a
  line). This was a former Go divergence (resolved; divergences live in git
  history only — see `divergences.md`).
- **charset/BOM** — compressed output prepends `\u{FEFF}`; expanded prepends
  `@charset "UTF-8";`.

`sourcesContent` is emitted only when `sourceMapIncludeSources` is set
(`CompileOptions.include_source_map_sources` threads through
`SerializeOptions` to `build_sources_content(include)`).

## SingleMapping

`SingleMapping` (in `compile/result.rs`) is the source-map value carried on
`CompileResult`:

```rust
impl SingleMapping {
    pub fn json(&self) -> SassResult<Vec<u8>>;                       // V3 JSON bytes
    pub fn json_with_target(&self, target: Option<String>) -> SassResult<Vec<u8>>;
}
```

`json()` emits no `file` key (the caller supplies the target via
`json_with_target`).

## SourceMapBuffer

`SourceMapBuffer` (see `serialize.md`) is `Plain` (no mapping) or `Mapping`
(records `(line, col)` and forwards entries to the builder). During
serialization, `for_span` maps the current output position to a source span
start, and each `\n` while in-span auto-maps the output line.

## File mapping

| Dart                                   | Go                          | Rust                       |
| -------------------------------------- | --------------------------- | -------------------------- |
| `lib/src/util/source_map_buffer.dart`  | `go/sourcemapbuffer/*.go`   | `src/source_map_buffer.rs` |
| `(external) package:source_maps` (VLQ) | `go/sourcemap/vlq.go`       | `src/sourcemap/vlq.rs`     |
| `lib/src/util/source_map_buffer.dart`  | `go/sourcemap/sourcemap.go` | `src/sourcemap/mod.rs`     |
