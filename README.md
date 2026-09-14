# rust-sass

`rust-sass` is a Rust port of [Dart Sass](https://sass-lang.com/dart-sass), the
reference implementation of the Sass CSS preprocessor. It produces
**byte-identical output** to Dart Sass `1.104.0` — the same CSS, source maps,
error messages, and warnings — while compiling to native binaries and
WebAssembly.

It is a single-threaded, arena-allocated compiler with **no `unsafe` code**,
with sync and async builds, and a drop-in replacement for the `sass` command
line, the `sass` npm package, the `sass-embedded` protocol server, and `libsass`
(C API — see [`docs/ref/libsass.md`](docs/ref/libsass.md)).

There is also a matching [Go port](https://github.com/bancek/go-sass).

The compiler is complete and passes the full spec suite against Dart Sass
`1.104.0` (14263/14263, plus the wasm spec via `bin/sass.js` and `js-api-spec`
197/204). The native CLI is faster than Dart Sass on every measured workload
(~0.83× CPU time on Bootstrap CSS).

Try it in the browser: [playground](https://bancek.github.io/rust-sass)
(runs the wasm build locally — live CSS, source maps, and warnings/errors).

## Workspace crates

Entry points are [`rust-sass`](rust-sass/) (the compiler library),
[`rust-sass-cli`](rust-sass-cli/) (the `rust-sass` command-line binary),
[`rust-sass-embedded`](rust-sass-embedded/) (the `sass-embedded` protocol
server), [`rust-sass-wasm`](rust-sass-wasm/) (the JS API compiled to
WebAssembly), and [`rust-sass-libsass`](rust-sass-libsass/) (the libsass C
API).

The full workspace layout lives in
[`docs/architecture.md §14`](docs/architecture.md#14-workspace-layout).

## Usage

### Library

```rust
use std::rc::Rc;
use bumpalo::Bump;
use rust_sass::compile::{compile_string, CompileOptions};
use rust_sass::io::{DefaultIo, Io};

let arena = Bump::new();
let io: Rc<dyn Io> = Rc::new(DefaultIo::new());
let options = CompileOptions::new(&arena);
let result = compile_string("$x: 1; a { b: $x; }", io, options, &arena)?;
assert_eq!(result.css(), "a {\n  b: 1;\n}");
```

The caller owns the arena (`Bump`) and the `Io` implementation; all values
produced by a compilation live in that arena for the duration of the call.

### Command line

```sh
cargo install rust-sass-cli
rust-sass input.scss output.css
```

Or build from a checkout without installing:

```sh
cargo build --release -p rust-sass-cli
target/release/rust-sass input.scss output.css
```

Pass `--help` for the full flag reference; the CLI mirrors the `sass` CLI
(`--style`, `--load-path`, `--source-map`, `--fatal-deprecation`, …). See
[`rust-sass-cli/README.md`](rust-sass-cli/README.md).

### Embedded protocol

```sh
rust-sass --embedded
```

Serves the Sass embedded protocol (version 3.2.0) over stdio, for use by the
`sass-embedded` host package. See [`docs/ref/embedded.md`](docs/ref/embedded.md).

### WebAssembly

`rust-sass-wasm` exposes the modern JS API (`compile`, `compileString`,
`compileAsync`, `Compiler`, importers, custom functions, the value classes) via
wasm-bindgen. See [`docs/ref/wasm.md`](docs/ref/wasm.md).

### libsass (C API)

`rust-sass-libsass` builds a drop-in `libsass.{so,dylib,a}` plus the pinned
`sass/*.h` headers, so existing native consumers (sassc, node-sass, language
bindings) link unmodified against the modern engine. See
[`docs/ref/libsass.md`](docs/ref/libsass.md).

## Building and testing

Prerequisites: Rust 1.92, a dev-channel Dart SDK (for the differential
research loop), submodules (`git clone --recurse-submodules`). See
[`docs/CONTRIBUTING.md`](docs/CONTRIBUTING.md#submodules) for submodule notes.

```sh
cargo build                              # whole workspace
cargo test -p rust-sass                  # unit tests, sync build
cargo test -p rust-sass --features async # unit tests, async build
cargo test -p rust-sass-spec --test runner_test --release -- --ignored  # sass-spec
cargo clippy --workspace                 # lint
```

Both sync and async modes must pass — they are one source tree compiled
twice. See [`docs/CONTRIBUTING.md`](docs/CONTRIBUTING.md) for the full
procedure (embedded differential, wasm, byte-identity gates, benchmarking).

## Upstream tracking

This port mirrors a specific dart-sass commit:

- [`PORTED_FROM`](PORTED_FROM) — the machine-readable tracked commit.
- [`docs/upstream.md`](docs/upstream.md) — the human-readable picture (commit,
  version, external-package pins).
- [`docs/porting.md`](docs/porting.md) — the procedure for porting new
  dart-sass changes.

Every ported source file carries a `// dart-source: lib/src/...` annotation
naming the Dart file it ports. This is the reverse index used when back-porting
upstream changes.

## Documentation

| Document                                                     | Contents                                                  |
| ------------------------------------------------------------ | --------------------------------------------------------- |
| [`docs/architecture.md`](docs/architecture.md)               | The pipeline and the design decisions behind it.          |
| [`docs/ref/pipeline.md`](docs/ref/pipeline.md)               | End-to-end walkthrough: arena ownership, entries, stages. |
| [`docs/patterns.md`](docs/patterns.md)                       | Translation conventions and the annotation system.        |
| [`docs/critical-invariants.md`](docs/critical-invariants.md) | Rules that must not be violated.                          |
| [`docs/ref/`](docs/ref/README.md)                            | Per-module reference (evaluator, parser, values, …).      |

Maintainer references:

| Document                                       | Contents                                                             |
| ---------------------------------------------- | -------------------------------------------------------------------- |
| [`docs/divergences.md`](docs/divergences.md)   | Intentional implementation differences (identical output); not bugs. |
| [`docs/porting.md`](docs/porting.md)           | How to port new dart-sass changes.                                   |
| [`docs/upstream.md`](docs/upstream.md)         | Tracked commit and external-package pins.                            |
| [`docs/CONTRIBUTING.md`](docs/CONTRIBUTING.md) | Build, test, and contribution guide.                                 |
| [`docs/ci.md`](docs/ci.md)                      | CI workflows, release tags, and roadmap.                            |
| [`docs/CHANGELOG.md`](docs/CHANGELOG.md)       | Release history.                                                     |
| [`docs/review.md`](docs/review.md)             | Rust↔Dart review guide (how to check a port for bugs).               |

## License

MIT. Copyright (c) 2026 Luka Zakrajšek <luka@bancek.net>.

Ported files carry the Dart source's own license header (Google MIT, or the BSD
"Dart project authors" header for files derived from third-party packages) plus
a `Ported and rearchitected for Rust by Luka Zakrajsek.` attribution line. See
[`docs/upstream.md`](docs/upstream.md) for the external-package inventory.
