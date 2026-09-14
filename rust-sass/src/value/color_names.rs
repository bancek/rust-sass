// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/color_names.dart
// go-source: go/value/color_names.go

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::common::exception::SassResult;
use crate::util::number;
use crate::value::color::{ColorSpace, SassColor};

/// 149 named CSS colors in reverse-alphabetical order (matching Dart's source).
/// Last-write-wins for the reverse lookup map means alphabetically-first names
/// win for duplicate RGB values (e.g., "gray" overwrites "grey").
// Canonical CSS data; a named alias would be single-use indirection.
#[allow(clippy::type_complexity)]
static COLOR_NAME_PAIRS: &[(&str, (u8, u8, u8, f64))] = &[
    ("yellowgreen", (154u8, 205u8, 50u8, 1f64)),
    ("yellow", (255u8, 255u8, 0u8, 1f64)),
    ("whitesmoke", (245u8, 245u8, 245u8, 1f64)),
    ("white", (255u8, 255u8, 255u8, 1f64)),
    ("wheat", (245u8, 222u8, 179u8, 1f64)),
    ("violet", (238u8, 130u8, 238u8, 1f64)),
    ("turquoise", (64u8, 224u8, 208u8, 1f64)),
    ("transparent", (0u8, 0u8, 0u8, 0f64)),
    ("tomato", (255u8, 99u8, 71u8, 1f64)),
    ("thistle", (216u8, 191u8, 216u8, 1f64)),
    ("teal", (0u8, 128u8, 128u8, 1f64)),
    ("tan", (210u8, 180u8, 140u8, 1f64)),
    ("steelblue", (70u8, 130u8, 180u8, 1f64)),
    ("springgreen", (0u8, 255u8, 127u8, 1f64)),
    ("snow", (255u8, 250u8, 250u8, 1f64)),
    ("slategrey", (112u8, 128u8, 144u8, 1f64)),
    ("slategray", (112u8, 128u8, 144u8, 1f64)),
    ("slateblue", (106u8, 90u8, 205u8, 1f64)),
    ("skyblue", (135u8, 206u8, 235u8, 1f64)),
    ("silver", (192u8, 192u8, 192u8, 1f64)),
    ("sienna", (160u8, 82u8, 45u8, 1f64)),
    ("seashell", (255u8, 245u8, 238u8, 1f64)),
    ("seagreen", (46u8, 139u8, 87u8, 1f64)),
    ("sandybrown", (244u8, 164u8, 96u8, 1f64)),
    ("salmon", (250u8, 128u8, 114u8, 1f64)),
    ("saddlebrown", (139u8, 69u8, 19u8, 1f64)),
    ("royalblue", (65u8, 105u8, 225u8, 1f64)),
    ("rosybrown", (188u8, 143u8, 143u8, 1f64)),
    ("red", (255u8, 0u8, 0u8, 1f64)),
    ("rebeccapurple", (102u8, 51u8, 153u8, 1f64)),
    ("purple", (128u8, 0u8, 128u8, 1f64)),
    ("powderblue", (176u8, 224u8, 230u8, 1f64)),
    ("plum", (221u8, 160u8, 221u8, 1f64)),
    ("pink", (255u8, 192u8, 203u8, 1f64)),
    ("peru", (205u8, 133u8, 63u8, 1f64)),
    ("peachpuff", (255u8, 218u8, 185u8, 1f64)),
    ("papayawhip", (255u8, 239u8, 213u8, 1f64)),
    ("palevioletred", (219u8, 112u8, 147u8, 1f64)),
    ("paleturquoise", (175u8, 238u8, 238u8, 1f64)),
    ("palegreen", (152u8, 251u8, 152u8, 1f64)),
    ("palegoldenrod", (238u8, 232u8, 170u8, 1f64)),
    ("orchid", (218u8, 112u8, 214u8, 1f64)),
    ("orangered", (255u8, 69u8, 0u8, 1f64)),
    ("orange", (255u8, 165u8, 0u8, 1f64)),
    ("olivedrab", (107u8, 142u8, 35u8, 1f64)),
    ("olive", (128u8, 128u8, 0u8, 1f64)),
    ("oldlace", (253u8, 245u8, 230u8, 1f64)),
    ("navy", (0u8, 0u8, 128u8, 1f64)),
    ("navajowhite", (255u8, 222u8, 173u8, 1f64)),
    ("moccasin", (255u8, 228u8, 181u8, 1f64)),
    ("mistyrose", (255u8, 228u8, 225u8, 1f64)),
    ("mintcream", (245u8, 255u8, 250u8, 1f64)),
    ("midnightblue", (25u8, 25u8, 112u8, 1f64)),
    ("mediumvioletred", (199u8, 21u8, 133u8, 1f64)),
    ("mediumturquoise", (72u8, 209u8, 204u8, 1f64)),
    ("mediumspringgreen", (0u8, 250u8, 154u8, 1f64)),
    ("mediumslateblue", (123u8, 104u8, 238u8, 1f64)),
    ("mediumseagreen", (60u8, 179u8, 113u8, 1f64)),
    ("mediumpurple", (147u8, 112u8, 219u8, 1f64)),
    ("mediumorchid", (186u8, 85u8, 211u8, 1f64)),
    ("mediumblue", (0u8, 0u8, 205u8, 1f64)),
    ("mediumaquamarine", (102u8, 205u8, 170u8, 1f64)),
    ("maroon", (128u8, 0u8, 0u8, 1f64)),
    ("magenta", (255u8, 0u8, 255u8, 1f64)),
    ("linen", (250u8, 240u8, 230u8, 1f64)),
    ("limegreen", (50u8, 205u8, 50u8, 1f64)),
    ("lime", (0u8, 255u8, 0u8, 1f64)),
    ("lightyellow", (255u8, 255u8, 224u8, 1f64)),
    ("lightsteelblue", (176u8, 196u8, 222u8, 1f64)),
    ("lightslategrey", (119u8, 136u8, 153u8, 1f64)),
    ("lightslategray", (119u8, 136u8, 153u8, 1f64)),
    ("lightskyblue", (135u8, 206u8, 250u8, 1f64)),
    ("lightseagreen", (32u8, 178u8, 170u8, 1f64)),
    ("lightsalmon", (255u8, 160u8, 122u8, 1f64)),
    ("lightpink", (255u8, 182u8, 193u8, 1f64)),
    ("lightgrey", (211u8, 211u8, 211u8, 1f64)),
    ("lightgreen", (144u8, 238u8, 144u8, 1f64)),
    ("lightgray", (211u8, 211u8, 211u8, 1f64)),
    ("lightgoldenrodyellow", (250u8, 250u8, 210u8, 1f64)),
    ("lightcyan", (224u8, 255u8, 255u8, 1f64)),
    ("lightcoral", (240u8, 128u8, 128u8, 1f64)),
    ("lightblue", (173u8, 216u8, 230u8, 1f64)),
    ("lemonchiffon", (255u8, 250u8, 205u8, 1f64)),
    ("lawngreen", (124u8, 252u8, 0u8, 1f64)),
    ("lavenderblush", (255u8, 240u8, 245u8, 1f64)),
    ("lavender", (230u8, 230u8, 250u8, 1f64)),
    ("khaki", (240u8, 230u8, 140u8, 1f64)),
    ("ivory", (255u8, 255u8, 240u8, 1f64)),
    ("indigo", (75u8, 0u8, 130u8, 1f64)),
    ("indianred", (205u8, 92u8, 92u8, 1f64)),
    ("hotpink", (255u8, 105u8, 180u8, 1f64)),
    ("honeydew", (240u8, 255u8, 240u8, 1f64)),
    ("grey", (128u8, 128u8, 128u8, 1f64)),
    ("greenyellow", (173u8, 255u8, 47u8, 1f64)),
    ("green", (0u8, 128u8, 0u8, 1f64)),
    ("gray", (128u8, 128u8, 128u8, 1f64)),
    ("goldenrod", (218u8, 165u8, 32u8, 1f64)),
    ("gold", (255u8, 215u8, 0u8, 1f64)),
    ("ghostwhite", (248u8, 248u8, 255u8, 1f64)),
    ("gainsboro", (220u8, 220u8, 220u8, 1f64)),
    ("fuchsia", (255u8, 0u8, 255u8, 1f64)),
    ("forestgreen", (34u8, 139u8, 34u8, 1f64)),
    ("floralwhite", (255u8, 250u8, 240u8, 1f64)),
    ("firebrick", (178u8, 34u8, 34u8, 1f64)),
    ("dodgerblue", (30u8, 144u8, 255u8, 1f64)),
    ("dimgrey", (105u8, 105u8, 105u8, 1f64)),
    ("dimgray", (105u8, 105u8, 105u8, 1f64)),
    ("deepskyblue", (0u8, 191u8, 255u8, 1f64)),
    ("deeppink", (255u8, 20u8, 147u8, 1f64)),
    ("darkviolet", (148u8, 0u8, 211u8, 1f64)),
    ("darkturquoise", (0u8, 206u8, 209u8, 1f64)),
    ("darkslategrey", (47u8, 79u8, 79u8, 1f64)),
    ("darkslategray", (47u8, 79u8, 79u8, 1f64)),
    ("darkslateblue", (72u8, 61u8, 139u8, 1f64)),
    ("darkseagreen", (143u8, 188u8, 143u8, 1f64)),
    ("darksalmon", (233u8, 150u8, 122u8, 1f64)),
    ("darkred", (139u8, 0u8, 0u8, 1f64)),
    ("darkorchid", (153u8, 50u8, 204u8, 1f64)),
    ("darkorange", (255u8, 140u8, 0u8, 1f64)),
    ("darkolivegreen", (85u8, 107u8, 47u8, 1f64)),
    ("darkmagenta", (139u8, 0u8, 139u8, 1f64)),
    ("darkkhaki", (189u8, 183u8, 107u8, 1f64)),
    ("darkgrey", (169u8, 169u8, 169u8, 1f64)),
    ("darkgreen", (0u8, 100u8, 0u8, 1f64)),
    ("darkgray", (169u8, 169u8, 169u8, 1f64)),
    ("darkgoldenrod", (184u8, 134u8, 11u8, 1f64)),
    ("darkcyan", (0u8, 139u8, 139u8, 1f64)),
    ("darkblue", (0u8, 0u8, 139u8, 1f64)),
    ("cyan", (0u8, 255u8, 255u8, 1f64)),
    ("crimson", (220u8, 20u8, 60u8, 1f64)),
    ("cornsilk", (255u8, 248u8, 220u8, 1f64)),
    ("cornflowerblue", (100u8, 149u8, 237u8, 1f64)),
    ("coral", (255u8, 127u8, 80u8, 1f64)),
    ("chocolate", (210u8, 105u8, 30u8, 1f64)),
    ("chartreuse", (127u8, 255u8, 0u8, 1f64)),
    ("cadetblue", (95u8, 158u8, 160u8, 1f64)),
    ("burlywood", (222u8, 184u8, 135u8, 1f64)),
    ("brown", (165u8, 42u8, 42u8, 1f64)),
    ("blueviolet", (138u8, 43u8, 226u8, 1f64)),
    ("blue", (0u8, 0u8, 255u8, 1f64)),
    ("blanchedalmond", (255u8, 235u8, 205u8, 1f64)),
    ("black", (0u8, 0u8, 0u8, 1f64)),
    ("bisque", (255u8, 228u8, 196u8, 1f64)),
    ("beige", (245u8, 245u8, 220u8, 1f64)),
    ("azure", (240u8, 255u8, 255u8, 1f64)),
    ("aquamarine", (127u8, 255u8, 212u8, 1f64)),
    ("aqua", (0u8, 255u8, 255u8, 1f64)),
    ("antiquewhite", (250u8, 235u8, 215u8, 1f64)),
    ("aliceblue", (240u8, 248u8, 255u8, 1f64)),
];

static NAME_BY_RGB: LazyLock<HashMap<(u8, u8, u8), &'static str>> = LazyLock::new(|| {
    let mut map = HashMap::with_capacity(149);
    for (name, (r, g, b, alpha)) in COLOR_NAME_PAIRS {
        if *alpha == 0.0 {
            continue;
        }
        map.insert((*r, *g, *b), *name);
    }
    map
});

// Single derived reverse-lookup map; a named alias would be single-use.
#[allow(clippy::type_complexity)]
static NAME_BY_NAME: LazyLock<HashMap<&'static str, (u8, u8, u8, f64)>> = LazyLock::new(|| {
    let mut map = HashMap::with_capacity(COLOR_NAME_PAIRS.len());
    for (name, val) in COLOR_NAME_PAIRS {
        let name_slice: &'static str = name;
        map.entry(name_slice).or_insert(*val);
    }
    map
});

/// Looks up a named color by (lowercase) name.
pub fn lookup_name(name: &str) -> Option<SassColor> {
    NAME_BY_NAME
        .get(name)
        .map(|(r, g, b, a)| SassColor::rgb(f64::from(*r), f64::from(*g), f64::from(*b), *a))
}

/// Returns the lowercase name of `c`, or `""` when no name matches.
///
/// Fully transparent black reports as `"transparent"`. Built from the
/// reverse-alphabetical table above so duplicate RGB values resolve to the
/// alphabetically-first name.
///
/// Matches Dart: namesByColor
pub fn color_name_for(c: &SassColor) -> SassResult<String> {
    let r = c.red()?;
    let g = c.green()?;
    let b = c.blue()?;
    let ri = r.round() as i32;
    let gi = g.round() as i32;
    let bi = b.round() as i32;
    let name = color_name_for_ints(ri, gi, bi).unwrap_or("");
    if name == "black" && (c.is_alpha_missing() || number::fuzzy_equals(c.alpha, 0.0)) {
        return Ok("transparent".to_string());
    }
    Ok(name.to_string())
}

/// Returns the lowercase name for integer `rgb` channels, or `None` when
/// out of range or unnamed.
///
/// Matches Dart: namesByColor
pub fn color_name_for_ints(r: i32, g: i32, b: i32) -> Option<&'static str> {
    if !(0..=255).contains(&r) || !(0..=255).contains(&g) || !(0..=255).contains(&b) {
        return None;
    }
    NAME_BY_RGB.get(&(r as u8, g as u8, b as u8)).copied()
}

// Fast path for already-`rgb`-space colors: only fuzzy-integer channels in
// `0..256` can have hex spellings, so anything else is unnamed ("").
//
// Matches Dart: namesByColor (the serializer's hex-shortening precheck).
#[allow(dead_code)]
pub(crate) fn color_name_for_sass_color(rgb: &SassColor) -> &'static str {
    if rgb.space != ColorSpace::Rgb {
        return "";
    }
    let r = rgb.channel0;
    let g = rgb.channel1;
    let b = rgb.channel2;
    if !can_use_hex_for_channel(r) || !can_use_hex_for_channel(g) || !can_use_hex_for_channel(b) {
        return "";
    }
    let r_int = r.round() as i32;
    let g_int = g.round() as i32;
    let b_int = b.round() as i32;
    if !(0..=255).contains(&r_int) || !(0..=255).contains(&g_int) || !(0..=255).contains(&b_int) {
        return "";
    }
    if number::fuzzy_equals(r, r_int as f64)
        && number::fuzzy_equals(g, g_int as f64)
        && number::fuzzy_equals(b, b_int as f64)
    {
        let name = NAME_BY_RGB
            .get(&(r_int as u8, g_int as u8, b_int as u8))
            .copied()
            .unwrap_or("");
        if name == "black" && (rgb.is_alpha_missing() || number::fuzzy_equals(rgb.alpha, 0.0)) {
            return "transparent";
        }
        name
    } else {
        ""
    }
}

// Whether a channel can round-trip through a hex spelling: a fuzzy integer
// in `0..256`.
//
// Matches Dart: fuzzyIsInt(channel) && fuzzyGreaterThanOrEquals(channel, 0) && fuzzyLessThan(channel, 256)
// Matches Go: FuzzyIsInt(channel) && FuzzyGreaterThanOrEquals(channel, 0) && FuzzyLessThan(channel, 256)
// Matches serialize/color.rs:390-394 (identical implementation)
fn can_use_hex_for_channel(v: f64) -> bool {
    // Matches Dart: fuzzyIsInt(channel) && fuzzyGreaterThanOrEquals(channel, 0) && fuzzyLessThan(channel, 256)
    // Matches Go: FuzzyIsInt(channel) && FuzzyGreaterThanOrEquals(channel, 0) && FuzzyLessThan(channel, 256)
    // Matches serialize/color.rs:390-394 (identical implementation)
    number::fuzzy_is_int(v)
        && number::fuzzy_greater_than_or_equals(v, 0.0)
        && number::fuzzy_less_than(v, 256.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_color_name_for_ints() {
        assert_eq!(color_name_for_ints(255, 0, 0), Some("red"));
        assert_eq!(color_name_for_ints(0, 0, 0), Some("black"));
        assert_eq!(color_name_for_ints(255, 255, 255), Some("white"));
        assert_eq!(color_name_for_ints(1, 2, 3), None);
    }

    #[test]
    fn test_color_name_for() {
        let c = SassColor::rgb(255.0, 0.0, 0.0, 1.0);
        let name = color_name_for(&c).unwrap();
        assert_eq!(name, "red");
    }

    #[test]
    fn test_gray_wins_over_grey() {
        assert_eq!(color_name_for_ints(0x80, 0x80, 0x80), Some("gray"));
    }
}
