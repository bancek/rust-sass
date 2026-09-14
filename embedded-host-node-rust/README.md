# sass-embedded-rust

The [`sass-embedded`](https://www.npmjs.com/package/sass-embedded) host API
backed by the [rust-sass](https://github.com/bancek/rust-sass) embedded compiler —
a drop-in replacement that compiles with Rust instead of Dart.

## Install

```sh
npm install sass-embedded-rust
```

## Usage

```js
import { compileString } from "sass-embedded-rust";

compileString("a {b: 1px + 2px}").css;
```

The API is identical to `sass-embedded` (same exports and types), only the
compiler binary differs. Both packages can be imported in one process without
interference.

## Versioning

Version tracks [dart-sass](https://github.com/sass/dart-sass) (`1.104.0`,
protocol `3.2.0`).

## Links

- [Repository](https://github.com/bancek/rust-sass) (compiler source).
- [Architecture and build docs](https://github.com/bancek/rust-sass/blob/main/docs/ref/sass-embedded-rust.md).

## License

MIT. Host code is © Google LLC (MIT).
