# rust-sass-cli

The `rust-sass` command-line compiler — a drop-in replacement for the `sass`
CLI. See `../README.md` for the project overview.

## Building

```sh
cargo build --release -p rust-sass-cli
# produces target/release/rust-sass
```

## Usage

```sh
rust-sass [options] [input.scss [output.css]]
rust-sass [options] input.scss:output.css
```

One or more sources may be passed; a source of `-` (or `--stdin`) reads from
stdin. A directory source compiles its non-partial `.scss`/`.sass`/`.css`
entrypoints to the corresponding destination directory.

### Options

| Flag                               | Description                                                             |
| ---------------------------------- | ----------------------------------------------------------------------- |
| `-I, --load-path PATH`             | A path for resolving imports (repeatable).                              |
| `-p, --pkg-importer TYPE`          | Built-in importer(s) for `pkg:` URLs (`node`).                          |
| `-s, --style NAME`                 | Output style: `expanded` (default) or `compressed`.                     |
| `--charset` / `--no-charset`       | Emit a `@charset`/BOM for non-ASCII output (default on).                |
| `--error-css`                      | Emit a stylesheet describing an error (default when writing to a file). |
| `--source-map` / `--no-source-map` | Generate source maps (default on).                                      |
| `--source-map-urls TYPE`           | `relative` (default) or `absolute`.                                     |
| `--embed-sources`                  | Embed source contents in source maps.                                   |
| `--embed-source-map`               | Embed source maps in CSS.                                               |
| `-q, --quiet`                      | Don't print warnings.                                                   |
| `--quiet-deps`                     | Don't print warnings from dependencies.                                 |
| `--verbose`                        | Print all deprecation warnings.                                         |
| `--fatal-deprecation DEP`          | Treat a deprecation (or a Sass version) as an error.                    |
| `--silence-deprecation DEP`        | Ignore a deprecation.                                                   |
| `--future-deprecation DEP`         | Opt into a deprecation early.                                           |
| `--stop-on-error`                  | Stop after the first error.                                             |
| `--trace`                          | Print full stack traces.                                                |
| `-c, --color`                      | Use terminal colors for messages.                                       |
| `--unicode` / `--no-unicode`       | Use Unicode glyphs in messages (default on).                            |

Exit codes: `64` usage error, `65` Sass error, `66` filesystem error.

### Embedded server mode

```sh
rust-sass --embedded
```

starts the Sass embedded protocol server (`../docs/ref/embedded.md`).

## License

MIT.
