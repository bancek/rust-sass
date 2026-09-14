# rust-sass-embedded-pb-gen

Regenerates the committed protobuf bindings for the embedded Sass compiler.

The generated file is committed at `../rust-sass-embedded-pb/src/embedded_sass.rs` so
that `rust-sass-embedded-pb` builds with only the `prost` runtime dependency —
consumers never need `protoc` or `prost-build`.

## Regenerate

```sh
cargo run -p rust-sass-embedded-pb-gen --features gen
```

This rewrites `rust-sass-embedded-pb/src/embedded_sass.rs` in place from
`../../build/language/spec/embedded_sass.proto`. Commit the resulting file
alongside any proto change.

## Why this crate exists

- `prost-build` is an **optional** dependency, enabled only by the `gen` feature.
  A plain `cargo build` never compiles or links it, so it stays out of the
  default dependency graph.
- The generator is a normal binary (`src/main.rs`), not a build script: nothing
  runs it automatically, and it only does anything when run explicitly with
  `--features gen`.
- Without `gen`, the binary prints the command above and exits 1.

## Why the generated code lives in its own crate

`rust-sass-embedded-pb` (the crate holding `embedded_sass.rs`) has **no
`rust-sass` dependency**. Because `rust-sass-embedded` depends on `rust-sass`,
any change to `rust-sass` previously forced the generated protobuf code to
recompile along with it. Splitting the generated code into a standalone crate
keeps it cached across `rust-sass` edits — it only rebuilds when the proto
itself changes.
