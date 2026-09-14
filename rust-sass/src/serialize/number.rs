// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/visitor/serialize.dart
// go-source: go/value/visitor_number.go

use crate::source_map_buffer::SourceMapBuffer;
use crate::util::number as num_util;

use crate::serialize::{OutputStyle, SerializeState};

impl SerializeState {
    /// Writes `v` without exponent notation and with at most
    /// [`SassNumber::precision`](crate::value::SassNumber) digits after the
    /// decimal point (Dart's `SerializeVisitor._writeNumber`, inspect and
    /// compressed modes taken from `self`).
    pub fn write_number(&self, buf: &mut SourceMapBuffer<'_>, v: f64) {
        let inspect = self.inspect;
        let compressed = matches!(self.style, OutputStyle::Compressed);
        num_util::write_number_to(buf, v, inspect, compressed).unwrap();
    }
}

/// Like [`SerializeState::write_number`], but returns a string rather than
/// writing to `buf` (Dart's `_writeNumberToString`).
pub fn write_number_to_string(v: f64) -> String {
    num_util::write_number_to_string(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serialize::SerializeVisitor;

    #[test]
    fn test_write_number_integer() {
        let mut sv = SerializeVisitor::new_plain(true, false);
        sv.inner.write_number(&mut sv.buffer, 42.0);
        assert_eq!(sv.into_string(), "42");
    }

    #[test]
    fn test_write_number_to_string() {
        assert_eq!(write_number_to_string(42.0), "42");
    }
}
