# rust-sass-libsass

The libsass C API implemented on `rust-sass`: a drop-in
`libsass.{so,dylib,a}` plus the pinned `sass/*.h` headers, so existing native
consumers (sassc, node-sass, language bindings) link unmodified. See
`../docs/ref/libsass.md` for the full contract and architecture.

## Contract

Links like libsass, behaves like Dart Sass — same symbols, signatures,
ownership rules, and error shapes (ABI-compatible); modern language semantics
underneath. Output diffs vs real libsass are expected and documented, not bugs.

## Building

```sh
cargo build --release -p rust-sass-libsass
# target/release/libsass.{so,dylib,a} (sass.dll on Windows)
```

Sync-only: the workspace `async` feature compiles this crate to an empty
artifact (deliberately unsupported).

## License

MIT.
