// Shared helpers for the differential suites.
//
// Both `differential.rs` (non-interactive) and `interactive.rs` (host-callback
// driving) compare our embedded server against the Dart embedded compiler (the
// golden reference), so they share the Dart discovery and the normalization
// that makes the comparison possible.

use prost::Message;
use std::path::Path;

use rust_sass_embedded::embedded_sass::OutboundMessage;

/// The command to run the Dart embedded compiler, or `None` if it isn't
/// available (e.g., CI without node_modules).
///
/// The path comes from the `DART_SASS_EMBEDDED` env var, or defaults to the
/// sass-embedded npm package's bundled Dart compiler under
/// `rust/rust-sass-wasm/node_modules`. Override with `DART_SASS_EMBEDDED` (full
/// path to the compiler executable).
pub fn dart_command() -> Option<Vec<String>> {
    let (exe, extra) = match std::env::var("DART_SASS_EMBEDDED") {
        Ok(cmd) => (cmd, vec![]),
        Err(_) => {
            let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            let base = root
                .join("rust/rust-sass-wasm/node_modules/sass-embedded-darwin-arm64/dart-sass/src");
            (
                base.join("dart").to_string_lossy().into_owned(),
                vec![base.join("sass.snapshot").to_string_lossy().into_owned()],
            )
        }
    };
    if !Path::new(&exe).exists() {
        return None;
    }
    let mut cmd = vec![exe];
    cmd.extend(extra);
    cmd.push("--embedded".to_string());
    Some(cmd)
}

/// Normalizes an outbound packet for byte comparison: re-encoding the decoded
/// message with prost canonicalizes away Dart's protobuf-library presence quirks
/// (Dart emits zero-valued proto3 scalars like `line: 0` that prost omits), so
/// identical decoded messages yield identical bytes.
pub fn normalize(id: u32, msg: &OutboundMessage) -> (u32, Vec<u8>) {
    (id, msg.encode_to_vec())
}
