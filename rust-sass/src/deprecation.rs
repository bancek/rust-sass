// Copyright 2024 Google LLC. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/deprecation.dart
// go-source: go/deprecation/deprecation.go

//! A deprecated feature in the language.
//!
//! Dart models this as an enum with one variant per deprecation (plus a
//! `future` constructor for deprecations not yet tied to a version); Rust
//! uses a plain struct with one public constant per deprecation, since the
//! set must be extensible from outside the crate. Use [`from_id`] to look up
//! a deprecation by its kebab-case id, and [`for_version`] for the set live
//! in or before a given version.

use std::fmt;

/// A deprecated feature in the language.
///
/// The `id` is the unique kebab-case identifier used on the command line;
/// `deprecated_in` is the Dart Sass version that first deprecated the feature
/// (`Some("0.0.0")` for deprecations predating versioning, `None` for
/// deprecations not tied to a release, such as [`USER_AUTHORED`]);
/// `description` is shown in CLI usage (`None` hides the deprecation from the
/// listing); `obsolete_in` is the version that fully removed the feature
/// (`None` while still only deprecated). `is_future` marks deprecations that
/// will occur in the future (Dart's `Deprecation.future` constructor); no
/// shipped deprecation sets it yet.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Deprecation {
    /// A unique ID for this deprecation in kebab case.
    ///
    /// This is used to refer to the deprecation on the command line.
    pub id: &'static str,
    /// The Dart Sass version this feature was first deprecated in.
    ///
    /// For deprecations that have existed in all versions of Dart Sass, this
    /// is `Some("0.0.0")`. For deprecations not related to a specific Sass
    /// version, this is `None`. (Dart exposes this as a parsed `Version`;
    /// Rust keeps the underlying version string.)
    pub deprecated_in: Option<&'static str>,
    /// A description of this deprecation, displayed in the CLI usage.
    ///
    /// If this is `None`, the deprecation is not listed.
    pub description: Option<&'static str>,
    /// The Dart Sass version this feature was fully removed in, making the
    /// deprecation obsolete. `None` while still only deprecated.
    pub obsolete_in: Option<&'static str>,
    /// Whether this deprecation will occur in the future.
    ///
    /// If this is true, `deprecated_in` is `None`, since the live version is
    /// not yet known.
    pub is_future: bool,
}

impl fmt::Display for Deprecation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.id)
    }
}

// ID constants (used in struct and from_id match)
const CALL_STRING_ID: &str = "call-string";
const ELSEIF_ID: &str = "elseif";
const MOZ_DOCUMENT_ID: &str = "moz-document";
const RELATIVE_CANONICAL_ID: &str = "relative-canonical";
const NEW_GLOBAL_ID: &str = "new-global";
const COLOR_MODULE_COMPAT_ID: &str = "color-module-compat";
const SLASH_DIV_ID: &str = "slash-div";
const BOGUS_COMBINATORS_ID: &str = "bogus-combinators";
const STRICT_UNARY_ID: &str = "strict-unary";
const FUNCTION_UNITS_ID: &str = "function-units";
const DUPLICATE_VAR_FLAGS_ID: &str = "duplicate-var-flags";
const NULL_ALPHA_ID: &str = "null-alpha";
const ABS_PERCENT_ID: &str = "abs-percent";
const FS_IMPORTER_CWD_ID: &str = "fs-importer-cwd";
const CSS_FUNCTION_MIXIN_ID: &str = "css-function-mixin";
const MIXED_DECLS_ID: &str = "mixed-decls";
const FEATURE_EXISTS_ID: &str = "feature-exists";
const COLOR_4_API_ID: &str = "color-4-api";
const COLOR_FUNCTIONS_ID: &str = "color-functions";
const LEGACY_JS_API_ID: &str = "legacy-js-api";
const IMPORT_ID: &str = "import";
const GLOBAL_BUILTIN_ID: &str = "global-builtin";
const TYPE_FUNCTION_ID: &str = "type-function";
const COMPILE_STRING_RELATIVE_URL_ID: &str = "compile-string-relative-url";
const MISPLACED_REST_ID: &str = "misplaced-rest";
const WITH_PRIVATE_ID: &str = "with-private";
const IF_FUNCTION_ID: &str = "if-function";
const FUNCTION_NAME_ID: &str = "function-name";
const ADJACENT_COMPOUNDS_ID: &str = "adjacent-compounds";
const USER_AUTHORED_ID: &str = "user-authored";
const CALC_INTERP_ID: &str = "calc-interp";

pub const CALL_STRING: Deprecation = Deprecation {
    id: CALL_STRING_ID,
    deprecated_in: Some("0.0.0"),
    description: Some("Passing a string directly to meta.call()."),
    obsolete_in: None,
    is_future: false,
};

pub const ELSEIF: Deprecation = Deprecation {
    id: ELSEIF_ID,
    deprecated_in: Some("1.3.2"),
    description: Some("@elseif."),
    obsolete_in: None,
    is_future: false,
};

pub const MOZ_DOCUMENT: Deprecation = Deprecation {
    id: MOZ_DOCUMENT_ID,
    deprecated_in: Some("1.7.2"),
    description: Some("@-moz-document."),
    obsolete_in: None,
    is_future: false,
};

pub const RELATIVE_CANONICAL: Deprecation = Deprecation {
    id: RELATIVE_CANONICAL_ID,
    deprecated_in: Some("1.14.2"),
    description: Some("Imports using relative canonical URLs."),
    obsolete_in: None,
    is_future: false,
};

pub const NEW_GLOBAL: Deprecation = Deprecation {
    id: NEW_GLOBAL_ID,
    deprecated_in: Some("1.17.2"),
    description: Some("Declaring new variables with !global."),
    obsolete_in: None,
    is_future: false,
};

pub const COLOR_MODULE_COMPAT: Deprecation = Deprecation {
    id: COLOR_MODULE_COMPAT_ID,
    deprecated_in: Some("1.23.0"),
    description: Some("Using color module functions in place of plain CSS functions."),
    obsolete_in: None,
    is_future: false,
};

pub const SLASH_DIV: Deprecation = Deprecation {
    id: SLASH_DIV_ID,
    deprecated_in: Some("1.33.0"),
    description: Some("/ operator for division."),
    obsolete_in: None,
    is_future: false,
};

pub const BOGUS_COMBINATORS: Deprecation = Deprecation {
    id: BOGUS_COMBINATORS_ID,
    deprecated_in: Some("1.54.0"),
    description: Some("Leading, trailing, and repeated combinators."),
    obsolete_in: None,
    is_future: false,
};

pub const STRICT_UNARY: Deprecation = Deprecation {
    id: STRICT_UNARY_ID,
    deprecated_in: Some("1.55.0"),
    description: Some("Ambiguous + and - operators."),
    obsolete_in: None,
    is_future: false,
};

pub const FUNCTION_UNITS: Deprecation = Deprecation {
    id: FUNCTION_UNITS_ID,
    deprecated_in: Some("1.56.0"),
    description: Some("Passing invalid units to built-in functions."),
    obsolete_in: None,
    is_future: false,
};

pub const DUPLICATE_VAR_FLAGS: Deprecation = Deprecation {
    id: DUPLICATE_VAR_FLAGS_ID,
    deprecated_in: Some("1.62.0"),
    description: Some("Using !default or !global multiple times for one variable."),
    obsolete_in: None,
    is_future: false,
};

pub const NULL_ALPHA: Deprecation = Deprecation {
    id: NULL_ALPHA_ID,
    deprecated_in: Some("1.62.3"),
    description: Some("Passing null as alpha in the Dart API."),
    obsolete_in: None,
    is_future: false,
};

pub const ABS_PERCENT: Deprecation = Deprecation {
    id: ABS_PERCENT_ID,
    deprecated_in: Some("1.65.0"),
    description: Some("Passing percentages to the Sass abs() function."),
    obsolete_in: None,
    is_future: false,
};

pub const FS_IMPORTER_CWD: Deprecation = Deprecation {
    id: FS_IMPORTER_CWD_ID,
    deprecated_in: Some("1.73.0"),
    description: Some("Using the current working directory as an implicit load path."),
    obsolete_in: None,
    is_future: false,
};

pub const CSS_FUNCTION_MIXIN: Deprecation = Deprecation {
    id: CSS_FUNCTION_MIXIN_ID,
    deprecated_in: Some("1.76.0"),
    description: Some("Function and mixin names beginning with --."),
    obsolete_in: Some("1.94.0"),
    is_future: false,
};

pub const MIXED_DECLS: Deprecation = Deprecation {
    id: MIXED_DECLS_ID,
    deprecated_in: Some("1.77.7"),
    description: Some("Declarations after or between nested rules."),
    obsolete_in: Some("1.92.0"),
    is_future: false,
};

pub const FEATURE_EXISTS: Deprecation = Deprecation {
    id: FEATURE_EXISTS_ID,
    deprecated_in: Some("1.78.0"),
    description: Some("meta.feature-exists"),
    obsolete_in: None,
    is_future: false,
};

pub const COLOR_4_API: Deprecation = Deprecation {
    id: COLOR_4_API_ID,
    deprecated_in: Some("1.79.0"),
    description: Some("Certain uses of built-in sass:color functions."),
    obsolete_in: None,
    is_future: false,
};

pub const COLOR_FUNCTIONS: Deprecation = Deprecation {
    id: COLOR_FUNCTIONS_ID,
    deprecated_in: Some("1.79.0"),
    description: Some("Using global color functions instead of sass:color."),
    obsolete_in: None,
    is_future: false,
};

pub const LEGACY_JS_API: Deprecation = Deprecation {
    id: LEGACY_JS_API_ID,
    deprecated_in: Some("1.79.0"),
    description: Some("Legacy JS API."),
    obsolete_in: None,
    is_future: false,
};

pub const IMPORT: Deprecation = Deprecation {
    id: IMPORT_ID,
    deprecated_in: Some("1.80.0"),
    description: Some("@import rules."),
    obsolete_in: None,
    is_future: false,
};

pub const GLOBAL_BUILTIN: Deprecation = Deprecation {
    id: GLOBAL_BUILTIN_ID,
    deprecated_in: Some("1.80.0"),
    description: Some("Global built-in functions that are available in sass: modules."),
    obsolete_in: None,
    is_future: false,
};

pub const TYPE_FUNCTION: Deprecation = Deprecation {
    id: TYPE_FUNCTION_ID,
    deprecated_in: Some("1.86.0"),
    description: Some("Functions named \"type\"."),
    obsolete_in: Some("1.92.0"),
    is_future: false,
};

pub const COMPILE_STRING_RELATIVE_URL: Deprecation = Deprecation {
    id: COMPILE_STRING_RELATIVE_URL_ID,
    deprecated_in: Some("1.88.0"),
    description: Some("Passing a relative url to compileString()."),
    obsolete_in: None,
    is_future: false,
};

pub const MISPLACED_REST: Deprecation = Deprecation {
    id: MISPLACED_REST_ID,
    deprecated_in: Some("1.91.0"),
    description: Some("A rest parameter before a positional or named parameter."),
    obsolete_in: None,
    is_future: false,
};

pub const WITH_PRIVATE: Deprecation = Deprecation {
    id: WITH_PRIVATE_ID,
    deprecated_in: Some("1.92.0"),
    description: Some("Configuring private variables in @use, @forward, or load-css()."),
    obsolete_in: None,
    is_future: false,
};

pub const IF_FUNCTION: Deprecation = Deprecation {
    id: IF_FUNCTION_ID,
    deprecated_in: Some("1.95.0"),
    description: Some("The Sass if($condition, $if-true, $if-false) function."),
    obsolete_in: None,
    is_future: false,
};

pub const FUNCTION_NAME: Deprecation = Deprecation {
    id: FUNCTION_NAME_ID,
    deprecated_in: Some("1.98.0"),
    description: Some("Uppercase reserved function names."),
    obsolete_in: None,
    is_future: false,
};

pub const ADJACENT_COMPOUNDS: Deprecation = Deprecation {
    id: ADJACENT_COMPOUNDS_ID,
    deprecated_in: Some("1.100.0"),
    description: Some("Adjacent compound selectors like `[class]a`."),
    obsolete_in: None,
    is_future: false,
};

/// Used for deprecations coming from user-authored code. Never tied to a
/// release, so both version fields are `None`.
pub const USER_AUTHORED: Deprecation = Deprecation {
    id: USER_AUTHORED_ID,
    deprecated_in: None,
    description: None,
    obsolete_in: None,
    is_future: false,
};

// Kept for compatibility although the name was never actually used
// (Dart marks it `@Deprecated`).
pub const CALC_INTERP: Deprecation = Deprecation {
    id: CALC_INTERP_ID,
    deprecated_in: None,
    description: None,
    obsolete_in: None,
    is_future: false,
};

const ALL: &[&Deprecation] = &[
    &CALL_STRING,
    &ELSEIF,
    &MOZ_DOCUMENT,
    &RELATIVE_CANONICAL,
    &NEW_GLOBAL,
    &COLOR_MODULE_COMPAT,
    &SLASH_DIV,
    &BOGUS_COMBINATORS,
    &STRICT_UNARY,
    &FUNCTION_UNITS,
    &DUPLICATE_VAR_FLAGS,
    &NULL_ALPHA,
    &ABS_PERCENT,
    &FS_IMPORTER_CWD,
    &CSS_FUNCTION_MIXIN,
    &MIXED_DECLS,
    &FEATURE_EXISTS,
    &COLOR_4_API,
    &COLOR_FUNCTIONS,
    &LEGACY_JS_API,
    &IMPORT,
    &GLOBAL_BUILTIN,
    &TYPE_FUNCTION,
    &COMPILE_STRING_RELATIVE_URL,
    &MISPLACED_REST,
    &WITH_PRIVATE,
    &IF_FUNCTION,
    &FUNCTION_NAME,
    &ADJACENT_COMPOUNDS,
    &USER_AUTHORED,
    &CALC_INTERP,
];

/// Returns the deprecation with the given kebab-case id, or `None` if none
/// exists.
pub fn from_id(id: &str) -> Option<&'static Deprecation> {
    match id {
        CALL_STRING_ID => Some(&CALL_STRING),
        ELSEIF_ID => Some(&ELSEIF),
        MOZ_DOCUMENT_ID => Some(&MOZ_DOCUMENT),
        RELATIVE_CANONICAL_ID => Some(&RELATIVE_CANONICAL),
        NEW_GLOBAL_ID => Some(&NEW_GLOBAL),
        COLOR_MODULE_COMPAT_ID => Some(&COLOR_MODULE_COMPAT),
        SLASH_DIV_ID => Some(&SLASH_DIV),
        BOGUS_COMBINATORS_ID => Some(&BOGUS_COMBINATORS),
        STRICT_UNARY_ID => Some(&STRICT_UNARY),
        FUNCTION_UNITS_ID => Some(&FUNCTION_UNITS),
        DUPLICATE_VAR_FLAGS_ID => Some(&DUPLICATE_VAR_FLAGS),
        NULL_ALPHA_ID => Some(&NULL_ALPHA),
        ABS_PERCENT_ID => Some(&ABS_PERCENT),
        FS_IMPORTER_CWD_ID => Some(&FS_IMPORTER_CWD),
        CSS_FUNCTION_MIXIN_ID => Some(&CSS_FUNCTION_MIXIN),
        MIXED_DECLS_ID => Some(&MIXED_DECLS),
        FEATURE_EXISTS_ID => Some(&FEATURE_EXISTS),
        COLOR_4_API_ID => Some(&COLOR_4_API),
        COLOR_FUNCTIONS_ID => Some(&COLOR_FUNCTIONS),
        LEGACY_JS_API_ID => Some(&LEGACY_JS_API),
        IMPORT_ID => Some(&IMPORT),
        GLOBAL_BUILTIN_ID => Some(&GLOBAL_BUILTIN),
        TYPE_FUNCTION_ID => Some(&TYPE_FUNCTION),
        COMPILE_STRING_RELATIVE_URL_ID => Some(&COMPILE_STRING_RELATIVE_URL),
        MISPLACED_REST_ID => Some(&MISPLACED_REST),
        WITH_PRIVATE_ID => Some(&WITH_PRIVATE),
        IF_FUNCTION_ID => Some(&IF_FUNCTION),
        FUNCTION_NAME_ID => Some(&FUNCTION_NAME),
        ADJACENT_COMPOUNDS_ID => Some(&ADJACENT_COMPOUNDS),
        USER_AUTHORED_ID => Some(&USER_AUTHORED),
        CALC_INTERP_ID => Some(&CALC_INTERP),
        _ => None,
    }
}

/// Returns the set of all non-obsolete deprecations first deprecated in or
/// before `version` (Dart's `forVersion`: `deprecatedIn <= version` and
/// `obsoleteIn == null`).
pub fn for_version(version: &str) -> Vec<&'static Deprecation> {
    let (major, minor, patch) = parse_version(version);
    let mut result = Vec::new();
    for d in ALL {
        let Some(v) = d.deprecated_in else {
            continue;
        };
        let (dmajor, dminor, dpatch) = parse_version(v);
        if d.obsolete_in.is_none()
            && !version_less_than(major, minor, patch, dmajor, dminor, dpatch)
        {
            result.push(*d);
        }
    }
    result
}

fn parse_version(v: &str) -> (u32, u32, u32) {
    let mut parts = v.splitn(3, '.');
    let major = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let patch = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    (major, minor, patch)
}

fn version_less_than(m1: u32, n1: u32, p1: u32, m2: u32, n2: u32, p2: u32) -> bool {
    if m1 != m2 {
        return m1 < m2;
    }
    if n1 != n2 {
        return n1 < n2;
    }
    p1 < p2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constants_exist() {
        for d in ALL {
            assert!(!d.id.is_empty(), "deprecation has empty ID");
        }
        assert_eq!(ALL.len(), 31);
    }

    #[test]
    fn test_from_id() {
        let d = from_id("call-string").unwrap();
        assert_eq!(d.id, "call-string");
        assert!(from_id("nonexistent").is_none());
    }

    #[test]
    fn test_display_returns_id() {
        assert_eq!(CALL_STRING.to_string(), "call-string");
        assert_eq!(ABS_PERCENT.to_string(), "abs-percent");
    }

    #[test]
    fn test_for_version() {
        let v = for_version("2.0.0");
        assert!(
            v.iter().any(|d| d.id == "call-string"),
            "ForVersion('2.0.0') should include call-string"
        );

        let v = for_version("99.0.0");
        for d in &v {
            assert!(
                d.obsolete_in.is_none(),
                "{} has obsolete_in but was included",
                d.id
            );
        }
    }

    #[test]
    fn test_for_version_empty_at_zero() {
        let v = for_version("0.0.0");
        let has = v
            .iter()
            .filter(|d| d.deprecated_in == Some("0.0.0"))
            .count();
        assert!(
            has >= 1,
            "0.0.0 should include call-string (deprecated_in=0.0.0)"
        );
    }

    #[test]
    fn test_parse_version() {
        assert_eq!(parse_version("1.23.45"), (1, 23, 45));
        assert_eq!(parse_version("2.0"), (2, 0, 0));
        assert_eq!(parse_version(""), (0, 0, 0));
        assert_eq!(parse_version("3"), (3, 0, 0));
    }

    #[test]
    fn test_version_less_than() {
        assert!(version_less_than(1, 0, 0, 2, 0, 0));
        assert!(!version_less_than(2, 0, 0, 1, 0, 0));
        assert!(version_less_than(1, 5, 0, 1, 10, 0));
        assert!(!version_less_than(1, 0, 0, 1, 0, 0));
        assert!(version_less_than(1, 0, 0, 1, 0, 1));
    }
}
