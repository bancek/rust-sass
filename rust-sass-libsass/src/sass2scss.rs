// Copyright 2012-2016 Sass Open Source Foundation. Use of this source code
// is governed by an MIT-style license that can be found in the LICENSE
// file or at https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// libsass-source: src/sass2scss.cpp (converter) + include/sass2scss.h (option constants)

//! Indented-Sass → SCSS text converter (`sass2scss`).
//!
//! A line-for-line behavioral port of libsass's `sass2scss.cpp` state machine
//! (indent-stack, comment modes, selector-vs-property heuristics, quirks and
//! all). Like the original this is std-only text processing: no AST, no calls
//! into `rust-sass` (a `parse+serialize` round-trip would yield compiled CSS
//! and destroy source constructs — plan D8).
//!
//! # Byte-fidelity notes
//!
//! The port works on bytes (`Vec<u8>`), mirroring C++ `std::string`, so
//! non-UTF-8 input passes through the same way. Two upstream crash paths are
//! hardened instead of mirrored (adapter harden-don't-mirror rule):
//!
//! - `std::string::find_*` with `pos > size()` throws `std::out_of_range`
//!   (uncaught → abort), e.g. a bare `@import` line with no URL, or a line
//!   ending in `\` inside quotes reaching `findCommentOpener`. The port
//!   returns "not found" there, leaving the line unconverted.
//! - Out-of-range `operator[]` reads (e.g. a trailing `@import ,` item)
//!   observe `0` (matching C++ `string[size] == '\0'`); reads past that also
//!   observe `0` instead of heap garbage.
//!
//! # Safety boundary
//!
//! The `extern "C"` entry point documents its pointer contract under
//! `# Safety`, hardens NULL to NULL (upstream would crash), truncates at the
//! first NUL like upstream's `strlen` conversion, and catches panics via
//! [`crate::base::guard`]. The pure functions are safe.

use std::ffi::{c_char, c_int, CStr};
use std::ptr;

use crate::base::{copy_bytes_nul, guard};

/// No-block-braces output: `a { color: red; }` on one line.
pub const SASS2SCSS_PRETTIFY_0: c_int = 0;
/// Closing brace stays on the last-declaration line.
pub const SASS2SCSS_PRETTIFY_1: c_int = 1;
/// Closing brace on its own line.
pub const SASS2SCSS_PRETTIFY_2: c_int = 2;
/// Both braces on their own lines.
pub const SASS2SCSS_PRETTIFY_3: c_int = 3;
/// Keep `//` silent comments verbatim in the output.
pub const SASS2SCSS_KEEP_COMMENT: c_int = 32;
/// Strip both `//` and `/* */` comments.
pub const SASS2SCSS_STRIP_COMMENT: c_int = 64;
/// Rewrite `//` comments as `/* */` blocks.
pub const SASS2SCSS_CONVERT_COMMENT: c_int = 128;

/// The C++ `std::string::npos` sentinel: "not found".
const NPOS: usize = usize::MAX;
/// `SASS2SCSS_FIND_WHITESPACE` (`sass2scss.h`): `" \t\n\v\f\r"`.
const FIND_WS: &[u8] = b" \t\n\x0b\x0c\r";
/// Lowercase ASCII letters plus `-`: the `isPseudoSelector` truncation set
/// (`find_first_not_of("abcdefghijklmnopqrstuvwxyz-ABCDEFGHIJKLMNOPQRSTUVWXYZ", 1)`).
const PSEUDO_CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz-ABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// Comment-parser state: mirrors `converter.comment` (`""` = parsing,
/// `"//"` = src comment, `"/*"` = css comment).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CommentState {
    /// Not in a comment (`""`).
    Parsing,
    /// In a `//` silent comment.
    Src,
    /// In a `/* */` loud comment (or a `//` under `CONVERT`, which counts as CSS).
    Css,
}

/// Converter state: mirrors `struct converter` (`sass2scss.h:63-85`).
struct Converter {
    /// Bitmask options (low 3 bits prettify; 32/64/128 comment flags).
    options: c_int,
    /// Current line looks like a selector (childless lines get ` {}`).
    selector: bool,
    /// Last meaningful char was `,` (suppresses `;`/`{`).
    comma: bool,
    /// Previous line had char data (can open `{` / needs `;`).
    property: bool,
    /// Line already ends with `;` (don't double it).
    semicolon: bool,
    /// Comment-parser state.
    comment: CommentState,
    /// Synthetic final unwind only.
    end_of_file: bool,
    /// Deferred newline/comment buffer.
    whitespace: Vec<u8>,
    /// Literal indent-prefix stack, base `""`.
    indents: Vec<Vec<u8>>,
}

impl Converter {
    /// Mirrors the init block (`sass2scss.cpp:844-854`).
    fn new(options: c_int) -> Self {
        Converter {
            options,
            selector: false,
            comma: false,
            property: false,
            semicolon: false,
            comment: CommentState::Parsing,
            end_of_file: false,
            whitespace: Vec::new(),
            indents: vec![Vec::new()],
        }
    }

    /// Stack top: mirrors `INDENT(converter)` (`converter.indents.top()`).
    fn indent(&self) -> &[u8] {
        // The base `""` is never popped (the pop-loop stops at equal length),
        // so the stack is never empty.
        self.indents.last().map_or(&[], Vec::as_slice)
    }
}

/// `PRETTIFY(converter)` (`sass2scss.cpp:40`): low 3 bits of options.
fn prettify(options: c_int) -> c_int {
    options - (options & 248)
}

/// `KEEP_COMMENT(converter)` (`:42`).
fn keep_comment(c: &Converter) -> bool {
    c.options & SASS2SCSS_KEEP_COMMENT == SASS2SCSS_KEEP_COMMENT
}

/// `STRIP_COMMENT(converter)` (`:43`).
fn strip_comment(c: &Converter) -> bool {
    c.options & SASS2SCSS_STRIP_COMMENT == SASS2SCSS_STRIP_COMMENT
}

/// `CONVERT_COMMENT(converter)` (`:44`).
fn convert_comment(c: &Converter) -> bool {
    c.options & SASS2SCSS_CONVERT_COMMENT == SASS2SCSS_CONVERT_COMMENT
}

/// `IS_PARSING(converter)` (`:50`).
fn is_parsing(c: &Converter) -> bool {
    c.comment == CommentState::Parsing
}

/// `IS_SRC_COMMENT(converter)` (`:52`): `//` that is not converted.
fn is_src_comment(c: &Converter) -> bool {
    c.comment == CommentState::Src && !convert_comment(c)
}

/// `IS_CSS_COMMENT(converter)` (`:53`): `/*`, or `//` under `CONVERT`.
fn is_css_comment(c: &Converter) -> bool {
    c.comment == CommentState::Css || (c.comment == CommentState::Src && convert_comment(c))
}

/// `closer` (`:56-61`): levels 0–1 → `" }"`, ≥2 → newline + indent + `}`.
fn closer(c: &Converter) -> Vec<u8> {
    // Upstream spells the 0 and ≤1 arms as separate (identical) branches;
    // collapsed here since they are exactly equivalent.
    if prettify(c.options) <= 1 {
        b" }".to_vec()
    } else {
        let mut out = b"\n".to_vec();
        out.extend_from_slice(c.indent());
        out.push(b'}');
        out
    }
}

/// `opener` (`:64-69`): 0 → `" { "`, 1–2 → `" {"`, 3 → newline + indent + `{`.
fn opener(c: &Converter) -> Vec<u8> {
    if prettify(c.options) == 0 {
        b" { ".to_vec()
    } else if prettify(c.options) <= 2 {
        b" {".to_vec()
    } else {
        let mut out = b"\n".to_vec();
        out.extend_from_slice(c.indent());
        out.push(b'{');
        out
    }
}

/// Reads byte `pos`, observing `0` past the end.
///
/// C++ `string[size]` is `'\0'`; reads past that are hardened to `0` (see
/// module docs — upstream reads heap garbage there).
fn byte_at(hay: &[u8], pos: usize) -> u8 {
    if pos < hay.len() {
        hay[pos]
    } else {
        0
    }
}

/// `substr(pos, len)` with clamping; `pos > len` hardens to empty (upstream
/// throws `out_of_range` — unreachable on exercised paths, see module docs).
fn substr(hay: &[u8], pos: usize, len: usize) -> &[u8] {
    if pos > hay.len() {
        return b"";
    }
    let end = pos.saturating_add(len).min(hay.len());
    &hay[pos..end]
}

/// `substr(pos)` to the end.
fn rest(hay: &[u8], pos: usize) -> &[u8] {
    substr(hay, pos, NPOS)
}

/// `find_first_of(set, pos)`: first index `>= pos` holding a byte from `set`.
/// `pos > len` hardens to not-found (upstream throws — see module docs).
fn find_first_of(hay: &[u8], set: &[u8], pos: usize) -> usize {
    if pos > hay.len() {
        return NPOS;
    }
    hay[pos..]
        .iter()
        .position(|b| set.contains(b))
        .map_or(NPOS, |i| pos + i)
}

/// `find_first_not_of(set, pos)`: first index `>= pos` holding a byte outside
/// `set`. Same hardening as [`find_first_of`].
fn find_first_not_of(hay: &[u8], set: &[u8], pos: usize) -> usize {
    if pos > hay.len() {
        return NPOS;
    }
    hay[pos..]
        .iter()
        .position(|b| !set.contains(b))
        .map_or(NPOS, |i| pos + i)
}

/// `find_last_not_of(set, pos)`: last index `<= pos` holding a byte outside
/// `set` (`pos` clamps to the end like C++, which defaults it to `npos`).
fn find_last_not_of(hay: &[u8], set: &[u8], pos: usize) -> usize {
    if hay.is_empty() {
        return NPOS;
    }
    let mut i = pos.min(hay.len() - 1);
    loop {
        if !set.contains(&hay[i]) {
            return i;
        }
        if i == 0 {
            return NPOS;
        }
        i -= 1;
    }
}

/// `find("*/", pos)`: substring search.
fn find_sub(hay: &[u8], needle: &[u8], pos: usize) -> usize {
    if pos > hay.len() || needle.len() > hay.len() {
        return NPOS;
    }
    (pos..=hay.len() - needle.len())
        .find(|&i| &hay[i..i + needle.len()] == needle)
        .unwrap_or(NPOS)
}

/// `isPseudoSelector` (`:73-155`): truncates at the first non-name char past
/// the leading `:`, lowercases, and matches the pseudo table (`:left`/`:right`
/// deliberately excluded — they are also valid properties).
fn is_pseudo_selector(sel: &mut Vec<u8>) -> bool {
    if sel.is_empty() {
        return false;
    }
    let pos = find_first_not_of(sel, PSEUDO_CHARS, 1);
    if pos != NPOS {
        sel.truncate(pos);
    }
    sel.make_ascii_lowercase();
    matches!(
        sel.as_slice(),
        b":link"
            | b":visited"
            | b":active"
            | b":lang"
            | b":first-child"
            | b":hover"
            | b":focus"
            | b":first"
            | b":target"
            | b":root"
            | b":nth-child"
            | b":nth-last-of-child"
            | b":nth-of-type"
            | b":nth-last-of-type"
            | b":last-child"
            | b":first-of-type"
            | b":last-of-type"
            | b":only-child"
            | b":only-of-type"
            | b":empty"
            | b":not"
            | b":default"
            | b":valid"
            | b":invalid"
            | b":in-range"
            | b":out-of-range"
            | b":required"
            | b":optional"
            | b":read-only"
            | b":read-write"
            | b":dir"
            | b":enabled"
            | b":disabled"
            | b":checked"
            | b":indeterminate"
            | b":nth-last-child"
            | b":any-link"
            | b":local-link"
            | b":scope"
            | b":active-drop-target"
            | b":valid-drop-target"
            | b":invalid-drop-target"
            | b":current"
            | b":past"
            | b":future"
            | b":placeholder-shown"
            | b":user-error"
            | b":blank"
            | b":nth-match"
            | b":nth-last-match"
            | b":nth-column"
            | b":nth-last-column"
            | b":matches"
            | b":fullscreen"
    )
}

/// `isUrl` (`:167-170`).
fn is_url(sass: &[u8], pos: usize) -> bool {
    byte_at(sass, pos) == b'u'
        && byte_at(sass, pos + 1) == b'r'
        && byte_at(sass, pos + 2) == b'l'
        && byte_at(sass, pos + 3) == b'('
}

/// `hasCharData` (`:174-206`): any meaningful char outside `/* */` comments.
fn has_char_data(sass: &[u8]) -> bool {
    let mut col = 0;
    loop {
        col = find_first_not_of(sass, FIND_WS, col);
        if col == NPOS {
            return false;
        }
        if substr(sass, col, 2) == b"/*" {
            col = find_sub(sass, b"*/", col);
            if col == NPOS {
                return false;
            }
            col += 2;
        } else {
            return true;
        }
    }
}

/// `findCommentOpener` (`:211-288`): quote/bracket-aware `//` search.
fn find_comment_opener(sass: &[u8]) -> usize {
    let mut col = 0;
    let mut apoed = false;
    let mut quoted = false;
    let mut comment = false;
    let mut brackets: usize = 0;
    while col != NPOS {
        col = find_first_of(sass, b"\"'/\\*()", col);
        if col != NPOS {
            match sass[col] {
                b'(' => {
                    if !quoted && !apoed {
                        brackets = brackets.wrapping_add(1);
                    }
                }
                b')' => {
                    if !quoted && !apoed {
                        brackets = brackets.wrapping_sub(1);
                    }
                }
                b'"' => {
                    if !apoed && !comment {
                        quoted = !quoted;
                    }
                }
                b'\'' => {
                    if !quoted && !comment {
                        apoed = !apoed;
                    }
                }
                b'/' => {
                    if col > 0 {
                        if byte_at(sass, col - 1) == b'*' {
                            comment = false;
                        } else if byte_at(sass, col - 1) == b'/'
                            && !quoted
                            && !apoed
                            && !comment
                            && brackets == 0
                        {
                            return col - 1;
                        }
                    }
                }
                b'\\' => {
                    if quoted || apoed {
                        col += 1;
                    }
                }
                b'*' => {
                    if col > 0 && byte_at(sass, col - 1) == b'/' && !quoted && !apoed {
                        comment = true;
                    }
                }
                _ => {}
            }
            col += 1;
        }
    }
    col
}

/// `removeMultilineComment` (`:292-365`): strips `/* */` outside quotes
/// (only used under `STRIP`).
fn remove_multiline_comment(sass: &[u8]) -> Vec<u8> {
    let mut clean = Vec::new();
    let mut col = 0;
    let mut open_pos = 0;
    let mut close_pos = 0;
    let mut apoed = false;
    let mut quoted = false;
    let mut comment = false;
    while col != NPOS {
        col = find_first_of(sass, b"\"'/\\*", col);
        if col != NPOS {
            match sass[col] {
                b'"' => {
                    if !apoed && !comment {
                        quoted = !quoted;
                    }
                }
                b'\'' => {
                    if !quoted && !comment {
                        apoed = !apoed;
                    }
                }
                b'/' => {
                    if comment && col > 0 && byte_at(sass, col - 1) == b'*' {
                        close_pos = col + 1;
                        comment = false;
                    }
                }
                b'\\' => {
                    if quoted || apoed {
                        col += 1;
                    }
                }
                b'*' => {
                    if !quoted && !apoed && col > 0 && byte_at(sass, col - 1) == b'/' {
                        comment = true;
                        open_pos = col - 1;
                        clean.extend_from_slice(substr(sass, close_pos, open_pos - close_pos));
                    }
                }
                _ => {}
            }
            col += 1;
        }
    }
    if comment {
        clean.extend_from_slice(rest(sass, open_pos));
    } else {
        clean.extend_from_slice(rest(sass, close_pos));
    }
    clean
}

/// `rtrim` (`:369-377`).
fn rtrim(sass: &[u8]) -> Vec<u8> {
    let pos = find_last_not_of(sass, FIND_WS, NPOS);
    if pos != NPOS {
        sass[..pos + 1].to_vec()
    } else {
        Vec::new()
    }
}

/// `flush` (`:381-452`): emits the whitespace buffer plus the line, splitting
/// off trailing `//` comments into the buffer.
fn flush(sass: &mut Vec<u8>, c: &mut Converter) -> Vec<u8> {
    let mut scss = Vec::new();
    if prettify(c.options) > 0 {
        scss.extend_from_slice(&c.whitespace);
    }
    c.whitespace.clear();

    let pos_right = find_last_not_of(sass, b"\n\r", NPOS);
    if pos_right == NPOS {
        return scss;
    }
    let lfs = rest(sass, pos_right + 1).to_vec();
    sass.truncate(pos_right + 1);

    let comment_pos = find_comment_opener(sass);
    if comment_pos != NPOS {
        if convert_comment(c) && is_parsing(c) {
            sass[comment_pos + 1] = b'*';
            sass.extend_from_slice(b" */");
        }
        let mut cp = comment_pos;
        if cp > 0 {
            let ws_pos = find_last_not_of(sass, FIND_WS, cp - 1);
            cp = if ws_pos == NPOS { 0 } else { ws_pos + 1 };
        }
        if !strip_comment(c) {
            c.whitespace.extend_from_slice(rest(sass, cp));
        }
        sass.truncate(cp);
    }

    c.whitespace.extend_from_slice(&lfs);
    c.whitespace.push(b'\n');

    if prettify(c.options) == 0 {
        let pos_left = find_first_not_of(sass, FIND_WS, 0);
        if pos_left != NPOS {
            sass.drain(..pos_left);
        }
    }

    scss.extend_from_slice(sass);
    scss
}

/// `process` (`:455-797`): converts one line, accumulating into the result.
fn process(sass: &mut Vec<u8>, c: &mut Converter) -> Vec<u8> {
    let mut scss = Vec::new();

    if strip_comment(c) {
        *sass = remove_multiline_comment(sass);
    }

    *sass = rtrim(sass);

    let mut pos_left = find_first_not_of(sass, FIND_WS, 0);

    if c.end_of_file {
        pos_left = 0;
    }

    if pos_left == NPOS {
        c.whitespace.extend_from_slice(sass);
        c.whitespace.push(b'\n');
    } else {
        let indent = substr(sass, 0, pos_left).to_vec();
        let open = substr(sass, pos_left, 2).to_vec();

        if indent.len() <= c.indent().len() {
            if is_css_comment(c) {
                if !strip_comment(c) {
                    scss.extend_from_slice(b" */");
                }
            } else if is_src_comment(c) {
                // Absorbed silently (the commented-out newline push is gone —
                // comments parse correctly now).
            } else if c.property && !c.comma {
                // Childless selectors get ` {}`, properties a missing `;`.
                if c.selector {
                    scss.extend_from_slice(b" {}");
                } else if !c.semicolon {
                    scss.extend_from_slice(b";");
                }
            }
            c.comment = CommentState::Parsing;
        }

        while indent.len() < c.indent().len() {
            c.indents.pop();
            if is_parsing(c) {
                scss.extend_from_slice(&closer(c));
            } else {
                scss.extend_from_slice(b" */");
            }
            c.comment = CommentState::Parsing;
        }

        c.selector = false;

        // Undocumented `\`-escape force-selector (mgreter/sass2scss#29).
        if substr(sass, pos_left, 1) == b"\\" {
            c.selector = true;
            sass[pos_left] = b' ';
        }

        if substr(sass, pos_left, 1) == b":" && substr(sass, pos_left, 2) != b"::" {
            c.selector = true;
            let pos_wspace = find_first_of(sass, FIND_WS, pos_left);
            if pos_wspace != NPOS {
                let mut pseudo = substr(sass, pos_left, pos_wspace - pos_left).to_vec();
                let pos_value = find_first_not_of(sass, FIND_WS, pos_wspace);
                if pos_value != NPOS
                    && !(byte_at(sass, pos_value) == b':' || is_pseudo_selector(&mut pseudo))
                {
                    let mut rewritten = indent.clone();
                    rewritten.extend_from_slice(substr(
                        sass,
                        pos_left + 1,
                        pos_wspace - pos_left - 1,
                    ));
                    rewritten.push(b':');
                    rewritten.extend_from_slice(rest(sass, pos_wspace));
                    *sass = rewritten;
                    let pos_colon = find_first_not_of(sass, b":", pos_left);
                    if pos_colon != NPOS {
                        let pc = find_first_of(sass, b":", pos_colon);
                        c.selector = pc == NPOS;
                    }
                }
            }

            // BEM property (one colon and no selector); note the length arg
            // is `pos_wspace` itself — an upstream quirk, mirrored blindly.
            if substr(sass, pos_left, 1) == b":" && c.selector {
                let pos_wspace = find_first_of(sass, FIND_WS, pos_left);
                let mut rewritten = indent.clone();
                rewritten.extend_from_slice(substr(sass, pos_left + 1, pos_wspace));
                rewritten.push(b':');
                *sass = rewritten;
            }
        } else if substr(sass, pos_left, 5) == b"@warn"
            || substr(sass, pos_left, 6) == b"@debug"
            || substr(sass, pos_left, 6) == b"@error"
            || substr(sass, pos_left, 6) == b"@value"
            || substr(sass, pos_left, 8) == b"@charset"
            || substr(sass, pos_left, 10) == b"@namespace"
        {
            let mut rewritten = indent.clone();
            rewritten.extend_from_slice(rest(sass, pos_left));
            *sass = rewritten;
        } else if substr(sass, pos_left, 1) == b"=" {
            let mut rewritten = indent.clone();
            rewritten.extend_from_slice(b"@mixin ");
            rewritten.extend_from_slice(rest(sass, pos_left + 1));
            *sass = rewritten;
        } else if substr(sass, pos_left, 1) == b"+" {
            if byte_at(sass, pos_left + 1) != 0
                && byte_at(sass, pos_left + 1) != b' '
                && byte_at(sass, pos_left + 1) != b'\t'
            {
                let mut rewritten = indent.clone();
                rewritten.extend_from_slice(b"@include ");
                rewritten.extend_from_slice(rest(sass, pos_left + 1));
                *sass = rewritten;
            }
        } else if substr(sass, pos_left, 7) == b"@import" {
            // Quote-aware `@import` auto-quoting (`:622-658`).
            let pos_import = find_first_of(sass, FIND_WS, pos_left + 7);
            let pos = find_first_not_of(sass, FIND_WS, pos_import);
            // Hardened: a bare `@import` (no URL) throws upstream
            // (`find_*` with `pos > size()`); the line is left alone.
            if pos_import != NPOS && pos != NPOS {
                let mut cur = pos;
                let mut start = pos;
                let mut in_dqstr = false;
                let mut in_sqstr = false;
                let mut is_escaped = false;
                loop {
                    let ch = byte_at(sass, cur);
                    if is_escaped {
                        is_escaped = false;
                    } else if ch == b'\\' {
                        is_escaped = true;
                    } else if ch == b'"' {
                        if !in_sqstr {
                            in_dqstr = !in_dqstr;
                        }
                    } else if ch == b'\'' {
                        if !in_dqstr {
                            in_sqstr = !in_sqstr;
                        }
                    } else if in_dqstr || in_sqstr {
                        // Skip over quoted sections.
                    } else if ch == b',' || ch == 0 {
                        // Hardened: a trailing empty item (`start == NPOS`)
                        // reads out of bounds upstream; it is left alone.
                        if start != NPOS
                            && byte_at(sass, start) != b'"'
                            && byte_at(sass, start) != b'\''
                            && !is_url(sass, start)
                        {
                            let end = find_last_not_of(sass, FIND_WS, cur.wrapping_sub(1))
                                .wrapping_add(1);
                            sass.insert(end.min(sass.len()), b'"');
                            sass.insert(start.min(sass.len()), b'"');
                            cur += 2;
                        }
                        start = find_first_not_of(sass, FIND_WS, cur + 1);
                    }
                    // `while (sass[pos++] != 0)`: the NUL at the end
                    // terminates after one final pass.
                    if ch == 0 {
                        break;
                    }
                    cur = cur.wrapping_add(1);
                }
            }
        } else if substr(sass, pos_left, 7) != b"@return"
            && substr(sass, pos_left, 7) != b"@extend"
            && substr(sass, pos_left, 8) != b"@include"
            && substr(sass, pos_left, 8) != b"@content"
        {
            // Generic selector-vs-property by colon+space (`:659-678`;
            // `#{}` colons count — colon blindness mirrored blindly).
            c.selector = true;
            let pos_colon = find_first_of(sass, b":", pos_left);
            if pos_colon != NPOS {
                if byte_at(sass, pos_colon + 1) == b' ' {
                    c.selector = false;
                }
                if byte_at(sass, pos_colon + 1) == b'\t' {
                    c.selector = false;
                }
            }
        }

        if indent.len() >= c.indent().len() && is_parsing(c) && has_char_data(sass) {
            c.property = true;
        }
        if indent.len() > c.indent().len() {
            if is_parsing(c) {
                if c.property {
                    scss.extend_from_slice(&opener(c));
                    c.indents.push(indent.clone());
                }
            } else if !is_css_comment(c) {
                // Multiline `//` continuation: re-slash the overlapped prefix
                // (`:713-723`). In bounds: `indent.len() > top` implies the
                // line is longer than `top + 1`.
                let top_len = c.indent().len();
                sass[top_len] = b'/';
                sass[top_len + 1] = b'/';
            }
        }

        if open == b"/*" || open == b"//" {
            c.property = false;
            if is_css_comment(c) && !open.is_empty() && !strip_comment(c) && !convert_comment(c) {
                scss.extend_from_slice(b" */");
            }
            if convert_comment(c) && is_parsing(c) {
                sass[pos_left + 1] = b'*';
            }
            c.comment = if open == b"//" {
                CommentState::Src
            } else {
                CommentState::Css
            };
        }

        if !((c.comment != CommentState::Parsing && strip_comment(c))
            || (is_src_comment(c) && !keep_comment(c)))
        {
            let flushed = flush(sass, c);
            scss.extend_from_slice(&flushed);
        }

        let pos_right = find_last_not_of(sass, FIND_WS, NPOS);
        if pos_right != NPOS {
            let close = byte_at(sass, pos_right);
            c.comma = is_parsing(c) && close == b',';
            c.semicolon = is_parsing(c) && close == b';';
            if pos_right > 0 && substr(sass, pos_right - 1, 2) == b"*/" {
                c.comment = CommentState::Parsing;
            }
        }
    }

    scss
}

/// `safeGetline` (`:801-832`) + drop-empty-tail: splits on CR, LF, and CRLF;
/// a trailing newline leaves no phantom line.
fn split_lines(sass: &[u8]) -> Vec<Vec<u8>> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < sass.len() {
        if sass[i] == b'\n' {
            lines.push(sass[start..i].to_vec());
            i += 1;
            start = i;
        } else if sass[i] == b'\r' {
            lines.push(sass[start..i].to_vec());
            i += 1;
            if i < sass.len() && sass[i] == b'\n' {
                i += 1;
            }
            start = i;
        } else {
            i += 1;
        }
    }
    if start < sass.len() {
        lines.push(sass[start..].to_vec());
    }
    lines
}

/// Pure converter: indented Sass bytes → SCSS bytes, byte-exact with upstream
/// `Sass::sass2scss(const std::string&, int)` (`:835-876`).
pub fn sass2scss_bytes(sass: &[u8], options: c_int) -> Vec<u8> {
    let mut c = Converter::new(options);
    let mut scss = Vec::new();
    for mut line in split_lines(sass) {
        scss.extend_from_slice(&process(&mut line, &mut c));
    }
    c.end_of_file = true;
    let mut eof = Vec::new();
    scss.extend_from_slice(&process(&mut eof, &mut c));
    scss
}

/// Pure converter over `&str` (all transforms are ASCII-only, so valid UTF-8
/// in yields valid UTF-8 out; the lossy fallback is unreachable).
pub fn sass2scss_str(sass: &str, options: c_int) -> String {
    let out = sass2scss_bytes(sass.as_bytes(), options);
    match String::from_utf8(out) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    }
}

/// Converts indented Sass to SCSS (`sass2scss.h:111`).
///
/// Mirrors the C wrapper (`sass2scss.cpp:885-888`): upstream converts the
/// input via `strlen`, so conversion stops at the first NUL. The result is a
/// fresh malloc'd buffer the caller frees with [`crate::base::sass_free_memory`].
/// NULL input hardens to NULL (upstream would crash).
///
/// # Safety
///
/// `sass` must be either NULL or a valid NUL-terminated string that remains
/// valid for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn sass2scss(sass: *const c_char, options: c_int) -> *mut c_char {
    guard(ptr::null_mut(), || {
        if sass.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: NULL checked; validity + NUL-termination per contract above.
        let bytes = unsafe { CStr::from_ptr(sass) }.to_bytes();
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        copy_bytes_nul(&sass2scss_bytes(&bytes[..end], options))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::sass_free_memory;
    use std::ffi::CString;

    /// Reads a malloc'd NUL-terminated C string into bytes (without taking
    /// ownership — ownership moves to the caller separately).
    unsafe fn read_c_str(ptr: *mut c_char) -> Vec<u8> {
        assert!(!ptr.is_null());
        // SAFETY: test-only; pointer is a valid NUL-terminated string produced
        // by the function under test.
        unsafe { CStr::from_ptr(ptr) }.to_bytes().to_vec()
    }

    /// Calls the FFI converter and returns the owned output, freeing the C
    /// allocation (mirrors `contract_base.rs read_owned`).
    unsafe fn call_ffi(sass: &[u8], options: c_int) -> Vec<u8> {
        let mut input = sass.to_vec();
        input.push(0);
        // SAFETY: test-only; NUL-terminated buffer live for the call, output
        // freed below.
        let got = unsafe { sass2scss(input.as_ptr() as *const c_char, options) };
        let out = unsafe { read_c_str(got) };
        unsafe {
            sass_free_memory(got as *mut std::ffi::c_void);
        }
        out
    }

    // All expectations below were captured from the upstream oracle binary
    // (`libsass/src/sass2scss.cpp` compiled standalone), never hand-written.

    const NEST: &str = "a\n  color: red\n  b\n    x: y\n";
    const NEST_CONVERTED_1: &[u8] = b"a {\n  color: red;\n  b {\n    x: y; } }\n";

    #[test]
    fn prettify_levels() {
        assert_eq!(
            sass2scss_str(NEST, SASS2SCSS_PRETTIFY_0),
            "a { color: red;b { x: y; } }"
        );
        assert_eq!(
            sass2scss_str(NEST, SASS2SCSS_PRETTIFY_1),
            "a {\n  color: red;\n  b {\n    x: y; } }\n"
        );
        assert_eq!(
            sass2scss_str(NEST, SASS2SCSS_PRETTIFY_2),
            "a {\n  color: red;\n  b {\n    x: y;\n  }\n}\n"
        );
        assert_eq!(
            sass2scss_str(NEST, SASS2SCSS_PRETTIFY_3),
            "a\n{\n  color: red;\n  b\n  {\n    x: y;\n  }\n}\n"
        );
    }

    const COMMENTS: &str = "// silent\na\n  color: red // trailing\n  /* loud */\n";

    #[test]
    fn comment_flag_matrix() {
        // Default: silent comments dropped, loud kept.
        assert_eq!(
            sass2scss_str(COMMENTS, SASS2SCSS_PRETTIFY_1),
            "a {\n  color: red; // trailing\n  /* loud */ }\n"
        );
        // KEEP: silent comments emitted verbatim.
        assert_eq!(
            sass2scss_str(COMMENTS, SASS2SCSS_PRETTIFY_1 | SASS2SCSS_KEEP_COMMENT),
            "// silent\na {\n  color: red; // trailing\n  /* loud */ }\n"
        );
        // STRIP: both comment kinds dropped.
        assert_eq!(
            sass2scss_str(COMMENTS, SASS2SCSS_PRETTIFY_1 | SASS2SCSS_STRIP_COMMENT),
            "a {\n  color: red; }\n\n"
        );
        // CONVERT: silent comments rewritten as loud blocks.
        assert_eq!(
            sass2scss_str(COMMENTS, SASS2SCSS_PRETTIFY_1 | SASS2SCSS_CONVERT_COMMENT),
            "/* silent */\na {\n  color: red; /* trailing */\n  /* loud */ }\n"
        );
    }

    #[test]
    fn comma_mixin_import_vectors() {
        assert_eq!(
            sass2scss_str("a,\nb\n  color: red\n", SASS2SCSS_PRETTIFY_1),
            "a,\nb {\n  color: red; }\n"
        );
        assert_eq!(
            sass2scss_str("=box\n  color: red\n+box\n", SASS2SCSS_PRETTIFY_1),
            "@mixin box {\n  color: red; }\n@include box;\n"
        );
        assert_eq!(
            sass2scss_str("@import foo, \"bar\", url(baz)\n", SASS2SCSS_PRETTIFY_1),
            "@import \"foo\", \"bar\", url(baz);\n"
        );
    }

    #[test]
    fn input_shape_edges() {
        // Empty input converts to empty output.
        assert_eq!(sass2scss_str("", SASS2SCSS_PRETTIFY_1), "");
        // No trailing newline still closes all blocks.
        assert_eq!(
            sass2scss_str("a\n  color: red", SASS2SCSS_PRETTIFY_1),
            "a {\n  color: red; }\n"
        );
        // CRLF line endings split like LF.
        assert_eq!(
            sass2scss_str(
                "a\r\n  color: red\r\n  b\r\n    x: y\r\n",
                SASS2SCSS_PRETTIFY_1
            ),
            "a {\n  color: red;\n  b {\n    x: y; } }\n"
        );
        // Lone-CR line endings split too.
        assert_eq!(
            sass2scss_str("a\r  color: red\r", SASS2SCSS_PRETTIFY_1),
            "a {\n  color: red; }\n"
        );
    }

    #[test]
    fn unknown_option_bits_ignored() {
        // Bits 3-4 and 8+ are not a prettify level or comment flag: the
        // PRETTIFY mask (`options & ~248`) drops them.
        let want = sass2scss_str(NEST, SASS2SCSS_PRETTIFY_1);
        assert_eq!(
            sass2scss_str(NEST, SASS2SCSS_PRETTIFY_1 | 8 | 16 | 224),
            want
        );
        assert_eq!(sass2scss_str(NEST, SASS2SCSS_PRETTIFY_1 | 224), want);
        // Combined comment flags behave as the OR of their bits.
        assert_eq!(
            sass2scss_str(COMMENTS, SASS2SCSS_PRETTIFY_1 | SASS2SCSS_KEEP_COMMENT),
            sass2scss_str(COMMENTS, 1 | 32)
        );
    }

    #[test]
    fn ffi_contract() {
        unsafe {
            // NULL input hardens to NULL (upstream would crash).
            assert!(sass2scss(ptr::null(), SASS2SCSS_PRETTIFY_1).is_null());
            // Basic conversion through the pointer boundary.
            assert_eq!(
                call_ffi(NEST.as_bytes(), SASS2SCSS_PRETTIFY_1),
                NEST_CONVERTED_1
            );
            // Caller frees with `sass_free_memory` (no leak; ownership moves
            // out — exercised by `call_ffi` on every vector above).
            let input = CString::new(NEST).unwrap();
            let got = sass2scss(input.as_ptr(), SASS2SCSS_PRETTIFY_1);
            assert!(!got.is_null());
            assert_ne!(got, input.as_ptr() as *mut c_char);
            sass_free_memory(got as *mut std::ffi::c_void);
            // Upstream `strlen` semantics: conversion stops at the first NUL.
            let mut nulled = b"a\n  x: y\n".to_vec();
            nulled.extend_from_slice(b"\x00garbage");
            assert_eq!(
                call_ffi(&nulled, SASS2SCSS_PRETTIFY_1),
                b"a {\n  x: y; }\n".to_vec()
            );
        }
    }
}
