# rust-sass-wasm

The [`sass`](https://www.npmjs.com/package/sass) JS API backed by the
[rust-sass](https://github.com/bancek/rust-sass) compiler running on
WebAssembly — a pure-JS install with no native binary and no Sass Embedded
Protocol.

If you want the native binary instead, use
[`sass-embedded-rust`](https://www.npmjs.com/package/sass-embedded-rust)
(the same compiler as an embedded-protocol host).

Try it in the browser: [playground](https://bancek.github.io/rust-sass)
(runs this build locally — live CSS, source maps, and warnings/errors).

## Install

```sh
npm install rust-sass-wasm
```

Requires Node.js `>=18.0.0`.

## Usage

```js
import { compileString } from "rust-sass-wasm";

compileString("a {b: 1px + 2px}").css;
```

```js
// CommonJS
const { compileString } = require("rust-sass-wasm");
```

String compilation (`compileString`, `compileStringAsync`) and file-based
compilation (`compile`, `compileAsync`) with importers, custom functions, the
`Value` classes, `Logger`, `deprecations`, `NodePackageImporter`, and the
`Compiler` API all work as in `sass`. The `sass` command line is included:

```sh
npx sass input.scss output.css
```

## Versioning

Version tracks [dart-sass](https://github.com/sass/dart-sass) (`1.104.0`).

## Links

- [Repository](https://github.com/bancek/rust-sass) (compiler source).
- [Architecture and build docs](https://github.com/bancek/rust-sass/blob/main/docs/ref/wasm.md).
- [Playground](https://bancek.github.io/rust-sass) (runs this build in the browser).

## License

MIT.
