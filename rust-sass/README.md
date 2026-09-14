# rust-sass

The `rust-sass` compiler library: a Rust port of [Dart Sass](https://sass-lang.com/dart-sass)
producing byte-identical output. See `../README.md` for the project overview
and `../docs/ref/` for the module reference.

## Quick start

```rust
use std::rc::Rc;
use bumpalo::Bump;
use rust_sass::compile::{compile_string, CompileOptions};
use rust_sass::io::{DefaultIo, Io};

let arena = Bump::new();
let io: Rc<dyn Io> = Rc::new(DefaultIo::new());
let options = CompileOptions::new(&arena);
let result = compile_string("a { b: 1 + 2; }", io, options, &arena)?;
println!("{}", result.css());
```

The caller owns the `Bump` arena and the `Io` implementation. All values
produced by a compilation live in the arena for the duration of the call; use
`CompileOptions::new(&arena)` so the default importer is arena-allocated.

## Public API

- `compile::{compile, compile_string, compile_stylesheet, CompileOptions, CompileResult, OutputStyle}`
- `io::{Io, IoExt, DefaultIo, VirtualIo, IoError}`
- `value::{Value, SassBoolean, SassString, SassNumber, SassColor, SassList, SassMap, SassArgumentList, SassCalculation, SassFunction, SassMixin, ListSeparator}`
- `callable::{Callable, CallableKind, BuiltInCallable, BuiltInCallback}`
- `eval::importer::{Importer, UserImporter, FilesystemImporter, NodePackageImporter, ImporterResult, CanonicalizeContext}`
- `logger::{Logger, QuietLogger, StderrLogger}`
- `common::exception::{SassError, SassResult}`
- `url::SassUrl`, `deprecation::{Deprecation, from_id}`, `Bump`

## Features

- `sync` (default) / `async` — the sync/async dual build (`async` flows to
  `rust-sass-macros`).
- `glibc-math` — use a Rust port of glibc's `e_pow.c` for `math::pow` (wasm
  parity; see `../docs/ref/math.md`).

## License

MIT. See `../README.md` for the header/attribution policy.
