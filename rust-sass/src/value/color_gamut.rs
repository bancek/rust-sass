// Copyright 2024 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/value/color/gamut_map_method.dart
// go-source: go/value/color_gamut.go

//! Re-exports the gamut-mapping method enum.
//!
//! Matches Dart: `gamut_map_method.dart` declares the `GamutMapMethod`
//! hierarchy (`clip`, `local-minde`, `fromName`, `map`); the enum itself and
//! its parsing live in `color_conversions_base.rs`, and the two mapping
//! algorithms live in `color_gamut_clip.rs` / `color_gamut_local_minde.rs`.

pub use crate::value::color_conversions_base::GamutMapMethod;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_name() {
        assert_eq!(
            GamutMapMethod::from_name("clip").unwrap(),
            GamutMapMethod::Clip
        );
        assert_eq!(
            GamutMapMethod::from_name("local-minde").unwrap(),
            GamutMapMethod::LocalMinde
        );
        assert!(GamutMapMethod::from_name("bogus").is_err());
    }

    #[test]
    fn test_as_str() {
        assert_eq!(GamutMapMethod::Clip.as_str(), "clip");
        assert_eq!(GamutMapMethod::LocalMinde.as_str(), "local-minde");
    }
}
