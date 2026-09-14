// Committed prost bindings for the embedded Sass protocol. Generated from
// build/language/spec/embedded_sass.proto by rust-sass-embedded-pb-gen.
// The generated file carries upstream proto-doc indentation that clippy
// flags (`doc_overindented_list_items`); allow it here rather than
// hand-editing generated code.
#[allow(clippy::doc_overindented_list_items)]
pub mod embedded_sass;

use std::fmt;

use embedded_sass::ProtocolError;

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Matches Go: ProtocolError.Error() returns GetMessage().
        f.write_str(&self.message)
    }
}
