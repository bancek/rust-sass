# rust-sass-macros

Proc macros powering rust-sass's **dual sync/async compilation**: one source
tree, two builds selected by cargo features. Inlined + patched copy of
[maybe-async](https://github.com/fMeow/maybe-async-rs) 0.2.11 (MIT) —
provenance and patch notes are in `src/lib.rs`.

Enable the `async` feature for the async build; the default sync build has
no async runtime dependency.

**Full reference: [`macros.md`](../docs/ref/macros.md)** — macro surface,
mode selection, pitfalls, and the sync-build failure triage table.

## License

MIT.
