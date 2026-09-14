# Module: `rust-sass-embedded`

The Sass embedded protocol server: a protocol-buffer-based server over
stdin/stdout that the `sass-embedded` host (Node.js) spawns with `--embedded`.

## What it is

Four crates implement it:

- `rust-sass-embedded` — the server library (`run`/version).
- `rust-sass-embedded-pb` — the committed prost-generated bindings
  (`embedded_sass.rs`), depending only on `prost` so it compiles once and stays
  cached.
- `rust-sass-embedded-pb-gen` — a build-only bin that regenerates the bindings.
- `rust-sass-cli` — the binary named `rust-sass` (its `--embedded` flag enters
  server mode).

It reports protocol version `3.2.0`, compiler/implementation version `1.104.0`,
and implementation name `"dart-sass"` — the values the `sass-embedded` host
requires.

## Wire protocol

Packets are **length-delimited**: `[varint length][varint compilation_id][protobuf message]`.
The compilation-ID varint is at most 32 bits; the length varint at most 53 bits.

- **compilation_id 0** → `VersionRequest` → `VersionResponse` (id = request id).
- **compilation_id ≠ 0** → the compilation's mailbox: a `CompileRequest` first
  (producing an `OutboundMessage.CompileResponse` — `CompileSuccess` or
  `CompileFailure` — on the same compilation id), then
  `CanonicalizeResponse`/`ImportResponse`/`FileImportResponse`/
  `FunctionCallResponse` correlated by request id `0`.
- **Host callbacks** are synchronous request/response within a compilation; the
  compiler sends `CanonicalizeRequest`/`ImportRequest`/`FileImportRequest`/
  `FunctionCallRequest` and blocks awaiting the response. `LogEvent`s are
  fire-and-forget. The `OutboundMessage.error` id is `ERROR_ID = 0xffffffff`
  when no request id applies.

Exit codes: `64` (extra CLI args), `66` (compile I/O), `70` (internal compiler
error), `76` (host-caused protocol error). A protocol error writes the
`ProtocolError` packet to the host and **then** exits `70`/`76`; a compile
_failure_ is a normal `CompileFailure` response and must **not** exit.

## Decisions

| #   | Decision                                                                                                                                                                                         |
| --- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| D1  | Four-crate layout (avoids a dependency cycle; the pb crate is dependency-free so it caches).                                                                                                     |
| D2  | `prost` with **committed** generated code — no build-time `protoc`.                                                                                                                              |
| D3  | Reader thread + dispatcher thread + per-compilation threads; `block_on` per compilation; crossbeam at the dispatcher, `futures::channel` for the waker-capable per-compilation mailbox.          |
| D4  | `max_concurrent_compilations = 15` (fidelity; compilations block on host callbacks, so oversubscription is fine).                                                                                |
| D5  | `"dart-sass"` / `"1.104.0"` hardcoded.                                                                                                                                                           |
| D6  | Host function signatures never panic — use `new_async` + `parse_parameter_list` (non-panicking) rather than `function_async` (which panics), so an invalid signature becomes a `CompileFailure`. |
| D7  | `UserImporter::canonicalize` takes `&mut CanonicalizeContext` (so importers can mark `containing_url` accessed, which affects caching).                                                          |

## Threading

A reader thread pushes raw packets; a dispatcher thread routes them; a
per-compilation thread owns the `!Send` compiler, `Bump` arena, `Rc<dyn Io>`
(`DefaultIo`), the host importers/functions/logger, and the response receiver,
running the async compile under `block_on`. Outbound writes take the
process-global stdout lock per full packet.

The dispatcher releases a compilation's slot **atomically** — the serialized
final packet, the id removal, and the pool-slot free happen in one `select!` arm
to avoid the dart-sass#2004 id-reuse race.

## prost API shape

Generated with prost 0.14: nested messages live in parent modules
(`inbound_message::VersionRequest`, `outbound_message::compile_response::CompileFailure`,
`value::String`, …), `optional` proto3 fields are `Option<T>`, top-level enums are
`#[repr(i32)]` with `as_str_name()`, and **`loaded_urls` lives on
`CompileResponse` — not on `CompileSuccess`**.

## Protofier

`protofier.rs` converts `Value ↔ proto` with a per-compilation context that
tracks argument-list ids (round-tripped by id, with a shared `keywords_accessed`
flag) and opaque function/mixin ids. Colors map through the missing-channel
bitmask; calculations validate names and arities.

## Codegen

```sh
cargo run -p rust-sass-embedded-pb-gen --features gen
```

rewrites `embedded_sass.rs` from `sass/spec/embedded_sass.proto` (in the
`sass` language-spec submodule).

## Gotchas

- prost does **not** flatten nested messages with `_` the way `protoc-gen-go`
  does — use the nested-module names.
- `protofy_span` only accepts `SourceSpanWithContext` (the owned boundary type);
  `FileSpan` never reaches the wire.
- `FileImporter` uses `FilesystemImporter::new_no_load_path`, not `new_cwd`
  (which sets a spurious load-path deprecation).
- `--version` outputs protojson (camelCase, 2-space indent, `"id": 0`).

## File mapping

| Dart                                      | Go                                | Rust                                         |
| ----------------------------------------- | --------------------------------- | -------------------------------------------- |
| `lib/src/embedded/*.dart`                 | `go/embedded/*.go`                | `rust-sass-embedded/src/*.rs`                |
| `build/language/spec/embedded_sass.proto` | `go/embedded/embedded_sass.pb.go` | `rust-sass-embedded-pb/src/embedded_sass.rs` |
