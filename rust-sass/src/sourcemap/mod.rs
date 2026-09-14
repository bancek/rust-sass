// Copyright 2018 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/util/source_map_buffer.dart (lower-level: V3 source map JSON builder)
// go-source: go/sourcemap/sourcemap.go

//! Version 3 source-map generation: the entry accumulator ([`Builder`]) and
//! the finished map value ([`SingleMapping`]).
//!
//! The low-level builder mirrors the `package:source_maps` `SingleMapping`
//! construction that Dart's `SourceMapBuffer` delegates to: entries are
//! accumulated per generated position, then encoded to VLQ mappings JSON on
//! demand. [`SingleMapping::json`] emits no `file` key (the caller supplies
//! the target via [`json_with_target`](SingleMapping::json_with_target)).

pub mod vlq;

use crate::common::SassError;
use std::collections::HashMap;

use serde::Serialize;

use crate::common::file_span::FileSpan;
use crate::common::source_span_file_source::FileSource;
use crate::common::SassResult;

use crate::sourcemap::vlq::encode_vlq;

/// A structured version 3 source map: the source URLs, the VLQ-encoded
/// `mappings` string, and the optional per-source contents (emitted as
/// `sourcesContent` only when `sourceMapIncludeSources` is set).
#[derive(Debug, Clone)]
pub struct SingleMapping {
    pub urls: Vec<String>,
    pub mappings: String,
    pub sources_content: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    gen_line: usize,
    gen_col: usize,
    source_idx: usize,
    src_line: usize,
    src_col: usize,
}

#[derive(Clone, Serialize)]
struct SourceMapJson {
    version: u32,
    #[serde(rename = "sourceRoot")]
    source_root: String,
    sources: Vec<String>,
    names: Vec<String>,
    mappings: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    #[serde(rename = "sourcesContent", skip_serializing_if = "Vec::is_empty")]
    sources_content: Vec<String>,
}

/// Accumulates `(generated line/column -> source position)` entries and
/// encodes them to V3 mappings on demand. Sources are deduplicated by URL in
/// first-seen order; entries are sorted by generated position at encode time.
pub struct Builder<'parse> {
    file: String,
    sources: Vec<String>,
    source_files: HashMap<usize, &'parse FileSource<'parse>>,
    entries: Vec<Entry>,
}

impl<'parse> Builder<'parse> {
    /// Creates an empty builder for the target file named `file` (used as
    /// the `file` key by [`to_json`](Self::to_json); [`SingleMapping::json`]
    /// omits it instead).
    pub fn new(file: String) -> Self {
        Builder {
            file,
            sources: Vec::new(),
            source_files: HashMap::new(),
            entries: Vec::new(),
        }
    }

    /// Records that the generated position maps to the start of `span`.
    pub fn add_mapping(
        &mut self,
        gen_line: usize,
        gen_col: usize,
        span: &FileSpan<'parse>,
    ) -> SassResult<()> {
        let source_url = span.source_url().map(|u| u.to_string()).unwrap_or_default();
        let source_idx = self
            .sources
            .iter()
            .position(|s| s == &source_url)
            .unwrap_or_else(|| {
                let idx = self.sources.len();
                self.sources.push(source_url);
                idx
            });

        if let std::collections::hash_map::Entry::Vacant(e) = self.source_files.entry(source_idx) {
            if let Some(file) = span.file() {
                e.insert(file);
            }
        }

        let start_loc = span.start_location();

        self.entries.push(Entry {
            gen_line,
            gen_col,
            source_idx,
            src_line: start_loc.line,
            src_col: start_loc.column,
        });
        Ok(())
    }

    /// Returns the target (line, column), source line, and source column of the
    /// last entry, if any.
    pub fn last_mapping(&self) -> Option<(usize, usize, usize, usize)> {
        self.entries
            .last()
            .map(|e| (e.gen_line, e.gen_col, e.src_line, e.src_col))
    }

    /// Removes the last entry if its target is exactly at `(gen_line, gen_col)`.
    ///
    /// Matches Dart: `SourceMapBuffer._writeLine`'s "Trim useless entries" step.
    pub fn trim_last_if_at(&mut self, gen_line: usize, gen_col: usize) {
        if let Some(e) = self.entries.last() {
            if e.gen_line == gen_line && e.gen_col == gen_col {
                self.entries.pop();
            }
        }
    }

    /// Adds an entry at `(gen_line, gen_col)` that reuses the last entry's
    /// source (used for the `in_span` continuation across a newline).
    ///
    /// Matches Dart: `SourceMapBuffer._writeLine`'s `Entry(_entries.last.source,
    /// _targetLocation)`.
    pub fn add_mapping_like_last(&mut self, gen_line: usize, gen_col: usize) {
        if let Some(e) = self.entries.last() {
            self.entries.push(Entry {
                gen_line,
                gen_col,
                source_idx: e.source_idx,
                src_line: e.src_line,
                src_col: e.src_col,
            });
        }
    }

    /// Encodes the accumulated entries to a [`SingleMapping`]. Source
    /// contents are included only when `include_source_content` is set
    /// (threaded from `CompileOptions.include_source_map_sources` through
    /// the serializer options).
    pub fn to_mapping(&self, include_source_content: bool) -> SassResult<SingleMapping> {
        if self.entries.is_empty() {
            return Ok(SingleMapping {
                urls: self.sources_or_empty(),
                mappings: String::new(),
                sources_content: self.build_sources_content(include_source_content),
            });
        }

        let mut sorted = self.entries.clone();
        sorted.sort_by(|a, b| a.gen_line.cmp(&b.gen_line).then(a.gen_col.cmp(&b.gen_col)));

        self.encode_entries(&sorted, include_source_content)
    }

    /// Like [`to_mapping`](Self::to_mapping), but shifts every entry forward
    /// past a `prefix` (line count and last-line column), for the
    /// `@charset`/BOM prefix. Matches Dart's
    /// `SourceMapBuffer.buildSourceMap(prefix:)`.
    pub fn to_mapping_with_prefix(
        &self,
        prefix_lines: usize,
        prefix_col: usize,
        include_source_content: bool,
    ) -> SassResult<SingleMapping> {
        if self.entries.is_empty() {
            return Ok(SingleMapping {
                urls: self.sources_or_empty(),
                mappings: String::new(),
                sources_content: self.build_sources_content(include_source_content),
            });
        }

        let mut sorted = self.entries.clone();
        for e in &mut sorted {
            if e.gen_line == 0 {
                e.gen_col += prefix_col;
            }
            e.gen_line += prefix_lines;
        }
        sorted.sort_by(|a, b| a.gen_line.cmp(&b.gen_line).then(a.gen_col.cmp(&b.gen_col)));

        self.encode_entries(&sorted, include_source_content)
    }

    fn encode_entries(
        &self,
        sorted: &[Entry],
        include_source_content: bool,
    ) -> SassResult<SingleMapping> {
        let mut mappings = String::new();
        let mut prev_gen_col: i32 = 0;
        let mut prev_src_idx: i32 = 0;
        let mut prev_src_line: i32 = 0;
        let mut prev_src_col: i32 = 0;
        let mut current_line: isize = -1;
        let mut first_on_line = true;

        for e in sorted {
            while current_line < e.gen_line as isize {
                if current_line >= 0 {
                    mappings.push(';');
                }
                current_line += 1;
                prev_gen_col = 0;
                first_on_line = true;
            }

            if !first_on_line {
                mappings.push(',');
            }
            first_on_line = false;

            let v = encode_vlq(e.gen_col as i32 - prev_gen_col).map_err(|e| SassError::Script {
                message: e,
                argument_name: None,
            })?;
            mappings.push_str(&v);
            prev_gen_col = e.gen_col as i32;

            let v =
                encode_vlq(e.source_idx as i32 - prev_src_idx).map_err(|e| SassError::Script {
                    message: e,
                    argument_name: None,
                })?;
            mappings.push_str(&v);
            prev_src_idx = e.source_idx as i32;

            let v =
                encode_vlq(e.src_line as i32 - prev_src_line).map_err(|e| SassError::Script {
                    message: e,
                    argument_name: None,
                })?;
            mappings.push_str(&v);
            prev_src_line = e.src_line as i32;

            let v = encode_vlq(e.src_col as i32 - prev_src_col).map_err(|e| SassError::Script {
                message: e,
                argument_name: None,
            })?;
            mappings.push_str(&v);
            prev_src_col = e.src_col as i32;
        }

        Ok(SingleMapping {
            urls: self.sources_or_empty(),
            mappings,
            sources_content: self.build_sources_content(include_source_content),
        })
    }

    /// Serializes the accumulated entries directly to V3 JSON bytes, setting
    /// the `file` key to the target name and always including
    /// `sourcesContent`. Prefer [`to_mapping`](Self::to_mapping) (plus
    /// [`SingleMapping::json`]) when the caller supplies the target.
    pub fn to_json(&self) -> SassResult<Vec<u8>> {
        if self.entries.is_empty() {
            let json = SourceMapJson {
                version: 3,
                source_root: String::new(),
                sources: self.sources_or_empty(),
                names: vec![],
                mappings: String::new(),
                file: Some(self.file.clone()),
                sources_content: Vec::new(),
            };
            return serde_json::to_vec(&json).map_err(|e| {
                Box::new(SassError::Script {
                    message: e.to_string(),
                    argument_name: None,
                })
            });
        }

        let mut sorted = self.entries.clone();
        sorted.sort_by(|a, b| a.gen_line.cmp(&b.gen_line).then(a.gen_col.cmp(&b.gen_col)));

        let mut mappings = String::new();
        let mut prev_gen_col: i32 = 0;
        let mut prev_src_idx: i32 = 0;
        let mut prev_src_line: i32 = 0;
        let mut prev_src_col: i32 = 0;
        let mut current_line: isize = -1;
        let mut first_on_line = true;

        for e in &sorted {
            while current_line < e.gen_line as isize {
                if current_line >= 0 {
                    mappings.push(';');
                }
                current_line += 1;
                prev_gen_col = 0;
                first_on_line = true;
            }

            if !first_on_line {
                mappings.push(',');
            }
            first_on_line = false;

            let v = encode_vlq(e.gen_col as i32 - prev_gen_col).map_err(|e| SassError::Script {
                message: e,
                argument_name: None,
            })?;
            mappings.push_str(&v);
            prev_gen_col = e.gen_col as i32;

            let v =
                encode_vlq(e.source_idx as i32 - prev_src_idx).map_err(|e| SassError::Script {
                    message: e,
                    argument_name: None,
                })?;
            mappings.push_str(&v);
            prev_src_idx = e.source_idx as i32;

            let v =
                encode_vlq(e.src_line as i32 - prev_src_line).map_err(|e| SassError::Script {
                    message: e,
                    argument_name: None,
                })?;
            mappings.push_str(&v);
            prev_src_line = e.src_line as i32;

            let v = encode_vlq(e.src_col as i32 - prev_src_col).map_err(|e| SassError::Script {
                message: e,
                argument_name: None,
            })?;
            mappings.push_str(&v);
            prev_src_col = e.src_col as i32;
        }

        let json = SourceMapJson {
            version: 3,
            source_root: String::new(),
            sources: self.sources_or_empty(),
            names: vec![],
            mappings,
            file: Some(self.file.clone()),
            sources_content: self.build_sources_content(true),
        };
        serde_json::to_vec(&json).map_err(|e| {
            Box::new(SassError::Script {
                message: e.to_string(),
                argument_name: None,
            })
        })
    }

    fn build_sources_content(&self, include: bool) -> Vec<String> {
        if !include || self.source_files.is_empty() || self.sources.is_empty() {
            return Vec::new();
        }
        let mut result = vec![String::new(); self.sources.len()];
        for (i, file) in &self.source_files {
            result[*i] = file.text().to_owned();
        }
        result
    }

    fn sources_or_empty(&self) -> Vec<String> {
        self.sources.clone()
    }
}

impl SingleMapping {
    /// Serializes this source map as V3 JSON with no `file` key (Dart's
    /// `SingleMapping.toJson` leaves `targetUrl` null; the embedded compiler
    /// passes `None` for the same reason).
    pub fn json(&self) -> SassResult<Vec<u8>> {
        self.json_with_target(None)
    }

    /// Serializes this source map as V3 JSON, setting the `file` key to
    /// [target] when provided. The embedded compiler passes `None` (no `file`
    /// key, matching Dart); the CLI passes the CSS basename.
    pub fn json_with_target(&self, target: Option<String>) -> SassResult<Vec<u8>> {
        let json = SourceMapJson {
            version: 3,
            source_root: String::new(),
            sources: self.urls.clone(),
            names: vec![],
            mappings: self.mappings.clone(),
            file: target,
            sources_content: self.sources_content.clone(),
        };
        serde_json::to_vec(&json).map_err(|e| {
            Box::new(SassError::Script {
                message: e.to_string(),
                argument_name: None,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::vlq::decode_vlq;
    use super::*;
    use crate::common::file_span::FileSpan;
    use crate::common::source_span_file_source::FileSource;
    use crate::url::SassUrl;
    use bumpalo::Bump;

    fn test_file_source<'compile, 'parse>(
        arena: &'compile Bump,
        text: &str,
        url_str: &str,
    ) -> &'parse FileSource<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let url = SassUrl::parse(url_str).unwrap();
        FileSource::new_in(arena, text, Some(url))
    }

    fn test_span<'compile, 'parse>(
        arena: &'compile Bump,
        text: &str,
        start_offset: usize,
        end_offset: usize,
    ) -> FileSpan<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = test_file_source(arena, text, "file:///input.scss");
        FileSpan::new(Some(fs), start_offset, end_offset)
    }

    fn test_source_span<'compile, 'parse>(
        arena: &'compile Bump,
        source_file: &str,
        line: usize,
        col: usize,
    ) -> FileSpan<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let content = "\n".repeat(line) + &"x".repeat(col);
        let offset = line + col;
        let fs = test_file_source(arena, &content, source_file);
        FileSpan::new(Some(fs), offset, offset)
    }

    #[test]
    fn test_builder_no_entries() {
        let b = Builder::new("output.css".into());
        let json_bytes = b.to_json().unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&json_bytes).unwrap();

        assert_eq!(parsed["version"], 3);
        assert_eq!(parsed["file"], "output.css");
        assert_eq!(parsed["mappings"], "");
        assert_eq!(parsed["sources"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn test_builder_single_entry() {
        let arena = Bump::new();
        let b = Builder::new("output.css".into());
        let mut b = b;
        b.add_mapping(0, 0, &test_span(&arena, "body { color: red; }", 0, 10))
            .unwrap();

        let json_bytes = b.to_json().unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&json_bytes).unwrap();

        assert_eq!(parsed["version"], 3);
        assert_eq!(parsed["file"], "output.css");
        assert_eq!(parsed["mappings"], "AAAA");

        let sources = parsed["sources"].as_array().unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0], "file:///input.scss");
    }

    #[test]
    fn test_builder_two_lines() {
        // gen(0,0)->src(line=2,col=5), gen(1,3)->src(line=5,col=10)
        // => golden "AAEK;GAGK"
        let arena = Bump::new();
        let mut b = Builder::new("output.css".into());
        b.add_mapping(0, 0, &test_source_span(&arena, "file:///input.scss", 2, 5))
            .unwrap();
        b.add_mapping(1, 3, &test_source_span(&arena, "file:///input.scss", 5, 10))
            .unwrap();

        let json_bytes = b.to_json().unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&json_bytes).unwrap();

        let got = parsed["mappings"].as_str().unwrap();
        let want = "AAEK;GAGK";
        assert_eq!(got, want, "mappings = {got:?}, want {want:?}");

        let mut pos = 0;
        let (col, np) = decode_vlq(got, pos).unwrap();
        pos = np;
        let (src_idx, np) = decode_vlq(got, pos).unwrap();
        pos = np;
        let (src_line, np) = decode_vlq(got, pos).unwrap();
        pos = np;
        let (src_col, np) = decode_vlq(got, pos).unwrap();
        pos = np;
        assert_eq!(
            (col, src_idx, src_line, src_col),
            (0, 0, 2, 5),
            "seg 1: col={col} srcIdx={src_idx} srcLine={src_line} srcCol={src_col}, want 0 0 2 5"
        );

        assert_eq!(got.as_bytes()[pos], b';');
        pos += 1;

        let (col, np) = decode_vlq(got, pos).unwrap();
        pos = np;
        let (src_idx, np) = decode_vlq(got, pos).unwrap();
        pos = np;
        let (src_line, np) = decode_vlq(got, pos).unwrap();
        pos = np;
        let (src_col, _np) = decode_vlq(got, pos).unwrap();
        assert_eq!(
            (col, src_idx, src_line, src_col),
            (3, 0, 3, 5),
            "seg 2: col={col} srcIdx={src_idx} srcLine={src_line} srcCol={src_col}, want 3 0 3 5"
        );
    }

    #[test]
    fn test_builder_same_line() {
        // gen(0,4)->src(line=1,col=4), gen(0,8)->src(line=3,col=2)
        // => golden "IACI,IAEF"
        let arena = Bump::new();
        let mut b = Builder::new("output.css".into());
        b.add_mapping(0, 4, &test_source_span(&arena, "file:///input.scss", 1, 4))
            .unwrap();
        b.add_mapping(0, 8, &test_source_span(&arena, "file:///input.scss", 3, 2))
            .unwrap();

        let json_bytes = b.to_json().unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&json_bytes).unwrap();

        let got = parsed["mappings"].as_str().unwrap();
        let want = "IACI,IAEF";
        assert_eq!(got, want, "mappings = {got:?}, want {want:?}");

        let mut pos = 0;
        let (col, np) = decode_vlq(got, pos).unwrap();
        pos = np;
        let (src_idx, np) = decode_vlq(got, pos).unwrap();
        pos = np;
        let (src_line, np) = decode_vlq(got, pos).unwrap();
        pos = np;
        let (src_col, np) = decode_vlq(got, pos).unwrap();
        pos = np;
        assert_eq!((col, src_idx, src_line, src_col), (4, 0, 1, 4));

        assert_eq!(got.as_bytes()[pos], b',');
        pos += 1;

        let (col, np) = decode_vlq(got, pos).unwrap();
        pos = np;
        let (src_idx, np) = decode_vlq(got, pos).unwrap();
        pos = np;
        let (src_line, np) = decode_vlq(got, pos).unwrap();
        pos = np;
        let (src_col, _np) = decode_vlq(got, pos).unwrap();
        assert_eq!((col, src_idx, src_line, src_col), (4, 0, 2, -2));
    }

    #[test]
    fn test_builder_multiple_sources() {
        // gen(0,0)->a.scss(0,0), gen(0,10)->b.scss(0,0)
        // => golden "AAAA,UCAA"
        let arena = Bump::new();
        let mut b = Builder::new("output.css".into());
        b.add_mapping(0, 0, &test_source_span(&arena, "file:///a.scss", 0, 0))
            .unwrap();
        b.add_mapping(0, 10, &test_source_span(&arena, "file:///b.scss", 0, 0))
            .unwrap();

        let json_bytes = b.to_json().unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&json_bytes).unwrap();

        let got = parsed["mappings"].as_str().unwrap();
        let want = "AAAA,UCAA";
        assert_eq!(got, want, "mappings = {got:?}, want {want:?}");

        let sources = parsed["sources"].as_array().unwrap();
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0], "file:///a.scss");
        assert_eq!(sources[1], "file:///b.scss");
    }

    #[test]
    fn test_builder_two_lines_different_sources() {
        // gen(0,0)->a.scss(0,0), gen(1,5)->src(line=2,col=3)
        // => golden "AAAA;KCEG"
        let arena = Bump::new();
        let mut b = Builder::new("output.css".into());
        b.add_mapping(0, 0, &test_source_span(&arena, "file:///a.scss", 0, 0))
            .unwrap();
        b.add_mapping(1, 5, &test_source_span(&arena, "file:///input.scss", 2, 3))
            .unwrap();

        let json_bytes = b.to_json().unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&json_bytes).unwrap();

        let got = parsed["mappings"].as_str().unwrap();
        let want = "AAAA;KCEG";
        assert_eq!(got, want, "mappings = {got:?}, want {want:?}");

        let sources = parsed["sources"].as_array().unwrap();
        assert_eq!(sources.len(), 2);
    }

    #[test]
    fn test_builder_multi_line() {
        // gen(0,2)->src(line=0,col=5)
        // gen(1,0)->src(line=1,col=3)
        // gen(1,8)->src(line=3,col=7)
        // => golden "EAAK;AACF,QAEI"
        let arena = Bump::new();
        let mut b = Builder::new("output.css".into());
        b.add_mapping(0, 2, &test_source_span(&arena, "file:///input.scss", 0, 5))
            .unwrap();
        b.add_mapping(1, 0, &test_source_span(&arena, "file:///input.scss", 1, 3))
            .unwrap();
        b.add_mapping(1, 8, &test_source_span(&arena, "file:///input.scss", 3, 7))
            .unwrap();

        let json_bytes = b.to_json().unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&json_bytes).unwrap();

        let got = parsed["mappings"].as_str().unwrap();
        let want = "EAAK;AACF,QAEI";
        assert_eq!(got, want, "mappings = {got:?}, want {want:?}");
    }

    #[test]
    fn test_single_mapping_json() {
        let m = SingleMapping {
            urls: vec!["file:///test.scss".into()],
            mappings: "AAAA".into(),
            sources_content: vec![],
        };
        let json_bytes = m.json().unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&json_bytes).unwrap();

        assert_eq!(parsed["version"], 3);
        assert!(
            !parsed.as_object().unwrap().contains_key("file"),
            "SingleMapping.JSON() should not include 'file'"
        );
        // Matches Dart's SingleMapping.toJson: always includes sourceRoot (empty).
        assert_eq!(parsed["sourceRoot"], "");
        let sources = parsed["sources"].as_array().unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0], "file:///test.scss");
        assert_eq!(parsed["mappings"], "AAAA");
        // Dart key order: version, sourceRoot, sources, names, mappings, sourcesContent.
        assert_eq!(
            String::from_utf8(json_bytes.clone()).unwrap(),
            "{\"version\":3,\"sourceRoot\":\"\",\"sources\":[\"file:///test.scss\"],\"names\":[],\"mappings\":\"AAAA\"}"
        );
    }

    #[test]
    fn test_single_mapping_json_with_sources_content() {
        let m = SingleMapping {
            urls: vec!["file:///test.scss".into()],
            mappings: "AAAA".into(),
            sources_content: vec!["body { color: red; }".into()],
        };
        let json_bytes = m.json().unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&json_bytes).unwrap();

        let sc = parsed["sourcesContent"].as_array().unwrap();
        assert_eq!(sc.len(), 1);
        assert_eq!(sc[0], "body { color: red; }");
        // sourcesContent comes last, after mappings.
        assert_eq!(
            String::from_utf8(json_bytes).unwrap(),
            "{\"version\":3,\"sourceRoot\":\"\",\"sources\":[\"file:///test.scss\"],\"names\":[],\"mappings\":\"AAAA\",\"sourcesContent\":[\"body { color: red; }\"]}"
        );
    }

    #[test]
    fn test_to_mapping_with_prefix() {
        let arena = Bump::new();
        let mut b = Builder::new("output.css".into());
        b.add_mapping(0, 0, &test_source_span(&arena, "file:///input.scss", 0, 0))
            .unwrap();

        let m = b.to_mapping_with_prefix(1, 5, true).unwrap();
        // gen(1,5)->src(0,0,0) => ";KAAA"
        assert_eq!(m.mappings, ";KAAA");
    }

    #[test]
    fn test_add_mapping_like_last_and_trim() {
        // Matches Dart `_writeLine`: the continuation entry reuses the last
        // entry's source, and a useless entry at the current position is
        // trimmed before the newline.
        let arena = Bump::new();
        let mut b = Builder::new("output.css".into());
        b.add_mapping(0, 0, &test_source_span(&arena, "file:///input.scss", 2, 5))
            .unwrap();
        b.add_mapping_like_last(1, 0);
        let m = b.to_mapping(true).unwrap();
        // gen(0,0)->src(2,5), then gen(1,0) reusing src(2,5): "AAEK;AAAA"
        assert_eq!(m.mappings, "AAEK;AAAA");

        // Trimming a non-matching position is a no-op.
        b.trim_last_if_at(1, 0);
        let m = b.to_mapping(true).unwrap();
        assert_eq!(m.mappings, "AAEK");

        // Trimming at the last entry's position removes it.
        b.add_mapping_like_last(2, 0);
        b.trim_last_if_at(2, 0);
        let m = b.to_mapping(true).unwrap();
        assert_eq!(m.mappings, "AAEK");
    }

    #[test]
    fn test_to_mapping_with_prefix_empty() {
        let b = Builder::new("output.css".into());
        let m = b.to_mapping_with_prefix(3, 10, true).unwrap();
        assert_eq!(m.mappings, "");
        assert!(m.urls.is_empty());
    }

    #[test]
    fn test_to_mapping_with_prefix_multi_line() {
        let arena = Bump::new();
        let mut b = Builder::new("output.css".into());
        b.add_mapping(0, 2, &test_source_span(&arena, "file:///input.scss", 0, 5))
            .unwrap();
        b.add_mapping(1, 0, &test_source_span(&arena, "file:///input.scss", 1, 3))
            .unwrap();

        let m = b.to_mapping_with_prefix(1, 0, true).unwrap();
        // After prefix: gen(1,2)->src(0,5), gen(2,0)->src(1,3)
        assert_eq!(m.mappings, ";EAAK;AACF");

        let json_bytes = m.json().unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&json_bytes).unwrap();
        assert_eq!(parsed["mappings"], ";EAAK;AACF");
    }

    #[test]
    fn test_builder_json_includes_sources_content() {
        let arena = Bump::new();
        let mut b = Builder::new("output.css".into());
        b.add_mapping(0, 0, &test_source_span(&arena, "file:///input.scss", 0, 0))
            .unwrap();

        let json_bytes = b.to_json().unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&json_bytes).unwrap();

        // Builder.ToJSON() includes sourcesContent from FileSource.text()
        // testSourceSpan creates content with "\n" * line + "x" * col
        // For line=0,col=0, content is empty
        let sc = parsed.get("sourcesContent");
        assert!(sc.is_some(), "sourcesContent should be present");
    }

    #[test]
    fn test_to_mapping_includes_sources_content() {
        let arena = Bump::new();
        let mut b = Builder::new("output.css".into());
        b.add_mapping(0, 0, &test_source_span(&arena, "file:///input.scss", 0, 0))
            .unwrap();

        let m = b.to_mapping(true).unwrap();
        assert_eq!(m.sources_content.len(), 1);
    }

    #[test]
    fn test_to_mapping_with_prefix_includes_sources_content() {
        let arena = Bump::new();
        let mut b = Builder::new("output.css".into());
        b.add_mapping(0, 0, &test_source_span(&arena, "file:///input.scss", 0, 0))
            .unwrap();

        let m = b.to_mapping_with_prefix(0, 0, true).unwrap();
        assert_eq!(m.sources_content.len(), 1);
    }

    #[test]
    fn test_to_mapping_sources_content_toggle() {
        let arena = Bump::new();
        let mut b = Builder::new("output.css".into());
        b.add_mapping(0, 0, &test_source_span(&arena, "file:///input.scss", 0, 0))
            .unwrap();

        let with = b.to_mapping(true).unwrap();
        assert_eq!(with.sources_content.len(), 1);

        let without = b.to_mapping(false).unwrap();
        assert_eq!(without.sources_content.len(), 0);
    }

    #[test]
    fn test_to_mapping_with_prefix_sources_content_toggle() {
        let arena = Bump::new();
        let mut b = Builder::new("output.css".into());
        b.add_mapping(0, 0, &test_source_span(&arena, "file:///input.scss", 0, 0))
            .unwrap();

        let m = b.to_mapping_with_prefix(1, 5, false).unwrap();
        assert_eq!(m.sources_content.len(), 0);
    }

    #[test]
    fn test_single_mapping_json_omits_empty_sources_content() {
        let m = SingleMapping {
            urls: vec!["file:///test.scss".into()],
            mappings: "AAAA".into(),
            sources_content: vec![],
        };
        let json_bytes = m.json().unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&json_bytes).unwrap();
        assert!(
            parsed.get("sourcesContent").is_none(),
            "sourcesContent should be omitted when empty"
        );
        assert_eq!(
            String::from_utf8(json_bytes).unwrap(),
            "{\"version\":3,\"sourceRoot\":\"\",\"sources\":[\"file:///test.scss\"],\"names\":[],\"mappings\":\"AAAA\"}"
        );
    }

    #[test]
    fn test_single_mapping_json_with_sources_content_toggle() {
        let with = SingleMapping {
            urls: vec!["file:///test.scss".into()],
            mappings: "AAAA".into(),
            sources_content: vec!["body { color: red; }".into()],
        };
        let parsed: serde_json::Value = serde_json::from_slice(&with.json().unwrap()).unwrap();
        assert!(parsed.get("sourcesContent").is_some());
    }
}
