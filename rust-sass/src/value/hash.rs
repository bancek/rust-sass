// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/utils.dart (listHash, mapHash) + package:collection hashCombine
// go-source: go/value/hash.go
//
// Hash helpers matching Dart's `==`/`hashCode` contracts for Sass values:
// list hashes fold element codes in order (package:collection
// `ListEquality`), map hashes combine per-entry codes without regard to
// insertion order (package:collection `MapEquality`).
//
/// Combines `value` into `seed` with the Boost-style `hash_combine` used
/// by `package:collection` (the same formula the list/map hashes build
/// on).
pub fn hash_combine(seed: i32, value: i32) -> i32 {
    seed ^ (value
        .wrapping_add(0x9e3779b9u32 as i32)
        .wrapping_add(seed << 6)
        .wrapping_add(seed >> 2))
}

/// Returns the hash code for `s` as a deterministic 31-fold over Unicode
/// scalar values. Quotes are folded by the caller (see `SassString`
/// `equals`/`hash_code`, which hash the text only).
pub fn string_hash_code(s: &str) -> i32 {
    let mut h: i32 = 0;
    for c in s.chars() {
        h = h.wrapping_mul(31).wrapping_add(c as i32);
    }
    h
}

/// Returns stable boolean hash codes (`true` → 1231, `false` → 1237),
/// mirroring the Java-style constants used by the Go port for
/// deterministic cross-run hashing.
pub fn bool_hash_code(v: bool) -> i32 {
    if v {
        1231
    } else {
        1237
    }
}

/// Returns a hash code for `values` that matches element-wise list
/// equality (Dart's `listHash`).
pub fn list_hash(values: &[i32]) -> i32 {
    let mut hash = 0;
    for v in values {
        hash = hash_combine(hash, *v);
    }
    hash
}

/// Returns a hash code for `entries` that matches order-independent map
/// equality (Dart's `mapHash`).
pub fn map_hash(entries: &[(i32, i32)]) -> i32 {
    // Matches Dart `mapHash` (`package:collection` `MapEquality.hash`):
    // unordered — per-entry `3*key + 7*value` summed, then the same
    // Jenkins-style finalizer the string/list hashes use. Order-independent
    // so `(a:1,b:2)` and `(b:2,a:1)` hash equally (they are `==`).
    const MASK: i64 = 0x7fffffff;
    let mut hash: i64 = 0;
    for (k, v) in entries {
        hash = (hash + 3 * (*k as i64) + 7 * (*v as i64)) & MASK;
    }
    hash = (hash + (hash << 3)) & MASK;
    hash ^= hash >> 11;
    hash = (hash + (hash << 15)) & MASK;
    hash as i32
}

/// Returns an identity-based hash for the referent of `p` (`None` → 0),
/// used for values whose Dart equality is object identity.
pub fn hash_ptr<T>(p: Option<&T>) -> i32 {
    match p {
        None => 0,
        Some(r) => {
            let addr = r as *const T as usize;
            addr as i32
        }
    }
}

/// Returns the hash code for an optional string (`None` → 0).
///
/// Rust-only helper with no Dart counterpart.
pub fn string_opt_hash_code(opt: &Option<String>) -> i32 {
    match opt {
        None => 0,
        Some(s) => string_hash_code(s),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_combine() {
        let h1 = hash_combine(1, 2);
        let h2 = hash_combine(1, 2);
        assert_eq!(h1, h2, "hash_combine must be deterministic");
        let h3 = hash_combine(2, 1);
        assert_ne!(h1, h3, "different inputs should produce different results");
    }

    #[test]
    fn test_string_hash_code() {
        assert_eq!(string_hash_code(""), 0);
        let h = string_hash_code("hello");
        assert_ne!(h, 0);
        assert_eq!(string_hash_code("hello"), string_hash_code("hello"));
        assert_ne!(string_hash_code("hello"), string_hash_code("Hello"));
    }

    #[test]
    fn test_bool_hash_code() {
        assert_eq!(bool_hash_code(true), 1231);
        assert_eq!(bool_hash_code(false), 1237);
    }

    #[test]
    fn test_list_hash() {
        assert_eq!(list_hash(&[]), 0);
        let h = list_hash(&[1, 2]);
        let expected = hash_combine(hash_combine(0, 1), 2);
        assert_eq!(h, expected);
    }

    #[test]
    fn test_map_hash() {
        assert_eq!(map_hash(&[]), 0);
        // Order-independent: swapped entries hash equally.
        assert_eq!(map_hash(&[(1, 10), (2, 20)]), map_hash(&[(2, 20), (1, 10)]));
        // Single-entry determinism.
        assert_eq!(map_hash(&[(1, 10)]), map_hash(&[(1, 10)]));
    }

    #[test]
    fn test_hash_ptr() {
        assert_eq!(hash_ptr::<i32>(None), 0);
        let x = 42;
        let h = hash_ptr(Some(&x));
        assert_ne!(h, 0);
        assert_eq!(hash_ptr(Some(&x)), h);
    }

    #[test]
    fn test_string_opt_hash_code_none() {
        assert_eq!(string_opt_hash_code(&None), 0);
    }

    #[test]
    fn test_string_opt_hash_code_some() {
        let opt = Some("hello".to_string());
        let expected = string_hash_code("hello");
        assert_eq!(string_opt_hash_code(&opt), expected);
    }

    #[test]
    fn test_string_opt_hash_code_deterministic() {
        let a = Some("foo".to_string());
        let b = Some("foo".to_string());
        assert_eq!(string_opt_hash_code(&a), string_opt_hash_code(&b));
    }
}
