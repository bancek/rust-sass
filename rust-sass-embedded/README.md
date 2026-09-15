# rust-sass-embedded

The Sass embedded protocol server: a protocol-buffer server over stdin/stdout,
spawned by the `sass-embedded` host package. See `../docs/ref/embedded.md` for
the full protocol and architecture.

## Running

```sh
rust-sass --embedded    # via rust-sass-cli
```

or programmatically via `rust_sass_embedded::run(args)`.

## Identity

The server reports protocol version `3.2.0`, compiler/implementation version
`1.104.0`, and implementation name `"dart-sass"` — the values the
`sass-embedded` host requires.

## License

MIT.
