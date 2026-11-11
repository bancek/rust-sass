// dart-source: sdk/lib/core/uri.dart (`Uri.file` / `toFilePath`) and
// package:path's toUri/fromUri (Dart 3.13.5).

//! Filesystem path ↔ `file:` URL conversion.

use super::{DartUriError, OwnedUrl};

const HEX: &[u8; 16] = b"0123456789ABCDEF";

const WINDOWS: bool = cfg!(windows);

/// Converts an absolute filesystem path to a `file:` URL, matching
/// `package:path`'s `absolutePathToUri` / `Uri.file`.
pub(super) fn absolute_path_to_uri(path: &str) -> Result<OwnedUrl, ()> {
    uri_file(path, WINDOWS)
}

/// `package:path`'s `toUri`: absolute paths become `file:` URLs; relative
/// paths become relative URI references.
#[cfg(test)]
pub(super) fn path_to_uri(path: &str, windows: bool) -> Result<OwnedUrl, ()> {
    let path = prepare_path(path, windows);
    if !is_absolute_path(&path, windows) {
        let separator = if windows { '\\' } else { '/' };
        let mut segments: Vec<String> = path
            .split(separator)
            .map(|segment| {
                let mut encoded = String::new();
                encode_segment(segment, &mut encoded);
                encoded
            })
            .collect();
        if let Some(first) = segments.first_mut() {
            *first = escape_scheme(first);
        }
        return Ok(OwnedUrl::make(
            String::new(),
            String::new(),
            None,
            None,
            segments.join("/"),
            None,
            None,
        ));
    }
    uri_file(&path, windows)
}

fn prepare_path(path: &str, windows: bool) -> String {
    let mut path = path.to_string();
    if windows {
        if let Some(stripped) = path.strip_prefix(r"\\?\") {
            if let Some(unc) = stripped.strip_prefix("UNC\\") {
                path = format!(r"\\{unc}");
            } else {
                path = stripped.to_string();
            }
        }
        path = path.replace('/', "\\");
    }
    normalize_path(&path, windows)
}

fn normalize_path(path: &str, windows: bool) -> String {
    let separator = if windows { '\\' } else { '/' };
    let (prefix, rest) = if windows {
        if let Some(rest) = path.strip_prefix(r"\\") {
            let mut parts = rest.splitn(3, '\\');
            let server = parts.next().unwrap_or("");
            let share = parts.next().unwrap_or("");
            let rest = parts.next().unwrap_or("");
            (format!(r"\\{server}\{share}"), rest.to_string())
        } else if path.len() >= 2 && path.as_bytes()[1] == b':' {
            let drive = &path[..2];
            if path.as_bytes().get(2) == Some(&b'\\') {
                (format!("{drive}\\"), path[3..].to_string())
            } else {
                (drive.to_string(), path[2..].to_string())
            }
        } else if let Some(rest) = path.strip_prefix('\\') {
            ("\\".to_string(), rest.to_string())
        } else {
            (String::new(), path.to_string())
        }
    } else if let Some(rest) = path.strip_prefix('/') {
        ("/".to_string(), rest.to_string())
    } else {
        (String::new(), path.to_string())
    };

    let trailing = rest.ends_with(separator);
    let mut stack: Vec<&str> = Vec::new();
    for segment in rest.split(separator) {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            if let Some(last) = stack.last() {
                if *last != ".." {
                    stack.pop();
                    continue;
                }
            }
            if prefix.is_empty() {
                stack.push("..");
            }
        } else {
            stack.push(segment);
        }
    }
    let mut result = prefix;
    if !result.is_empty() && !result.ends_with(separator) && !stack.is_empty() {
        result.push(separator);
    }
    result.push_str(&stack.join(separator.to_string().as_str()));
    if trailing && !stack.is_empty() && !result.ends_with(separator) {
        result.push(separator);
    }
    result
}

fn escape_scheme_path(path: &str) -> String {
    match path.split_once('/') {
        Some((first, rest)) => format!("{}/{}", escape_scheme(first), rest),
        None => escape_scheme(path),
    }
}

#[cfg(test)]
fn is_absolute_path(path: &str, windows: bool) -> bool {
    if windows {
        let bytes = path.as_bytes();
        path.starts_with('\\')
            || path.starts_with('/')
            || path.starts_with('\\')
            || (bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && (bytes[2] == b'\\' || bytes[2] == b'/'))
    } else {
        path.starts_with('/')
    }
}

fn escape_scheme(segment: &str) -> String {
    let bytes = segment.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() {
        for (index, byte) in bytes.iter().enumerate().skip(1) {
            if *byte == b':' {
                return format!("{}%3A{}", &segment[..index], &segment[index + 1..]);
            }
            if *byte > 127 || !is_scheme_char(*byte) {
                break;
            }
        }
    }
    segment.to_string()
}

fn is_scheme_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.')
}

pub(super) fn uri_file(path: &str, windows: bool) -> Result<OwnedUrl, ()> {
    let path = prepare_path(path, windows);
    if windows {
        windows_file_url(&path)
    } else {
        let segments: Vec<&str> = path.split('/').collect();
        if path.starts_with('/') {
            Ok(OwnedUrl::make(
                "file".to_string(),
                String::new(),
                Some(String::new()),
                None,
                encode_segments(&segments)?,
                None,
                None,
            ))
        } else {
            Ok(OwnedUrl::make(
                String::new(),
                String::new(),
                None,
                None,
                escape_scheme_path(&encode_segments(&segments)?),
                None,
                None,
            ))
        }
    }
}

fn windows_file_url(path: &str) -> Result<OwnedUrl, ()> {
    let mut path = path.to_string();
    if let Some(stripped) = path.strip_prefix(r"\\?\") {
        if let Some(unc) = stripped.strip_prefix("UNC\\") {
            path = format!(r"\{unc}");
        } else {
            path = stripped.to_string();
            let bytes = path.as_bytes();
            if path.len() < 3 || bytes[1] != b':' || bytes[2] != b'\\' {
                return Err(());
            }
        }
    }

    let bytes = path.as_bytes();
    if path.len() > 1 && bytes[1] == b':' {
        check_drive_letter(bytes[0])?;
        if path.len() == 2 || bytes[2] != b'\\' {
            return Err(());
        }
        let segments: Vec<&str> = path.split('\\').collect();
        check_windows_segments(&segments[1..])?;
        let mut encoded = encode_segments(&segments)?;
        if !encoded.starts_with('/') {
            encoded.insert(0, '/');
        }
        return Ok(OwnedUrl::make(
            "file".to_string(),
            String::new(),
            Some(String::new()),
            None,
            encoded,
            None,
            None,
        ));
    }

    if let Some(rest) = path.strip_prefix('\\') {
        if let Some(unc) = rest.strip_prefix('\\') {
            let path_start = unc.find('\\');
            let (host, path_part) = match path_start {
                Some(index) => (&unc[..index], &unc[index + 1..]),
                None => (unc, ""),
            };
            let segments: Vec<&str> = if path_part.is_empty() {
                Vec::new()
            } else {
                path_part.split('\\').collect()
            };
            check_windows_segments(&segments)?;
            let mut encoded = encode_segments(&segments)?;
            if !encoded.is_empty() && !encoded.starts_with('/') {
                encoded.insert(0, '/');
            }
            return Ok(OwnedUrl::make(
                "file".to_string(),
                String::new(),
                Some(host.to_string()),
                None,
                encoded,
                None,
                None,
            ));
        }
        let segments: Vec<&str> = path.split('\\').collect();
        check_windows_segments(&segments)?;
        let encoded = encode_segments(&segments)?;
        return Ok(OwnedUrl::make(
            "file".to_string(),
            String::new(),
            Some(String::new()),
            None,
            encoded,
            None,
            None,
        ));
    }

    let segments: Vec<&str> = path.split('\\').collect();
    check_windows_segments(&segments)?;
    Ok(OwnedUrl::make(
        String::new(),
        String::new(),
        None,
        None,
        escape_scheme_path(&encode_segments(&segments)?),
        None,
        None,
    ))
}

fn check_drive_letter(byte: u8) -> Result<(), ()> {
    if byte.is_ascii_alphabetic() {
        Ok(())
    } else {
        Err(())
    }
}

fn check_windows_segments(segments: &[&str]) -> Result<(), ()> {
    for segment in segments {
        if segment.bytes().any(|byte| {
            matches!(
                byte,
                b'"' | b'*' | b'/' | b':' | b'<' | b'>' | b'?' | b'\\' | b'|'
            )
        }) {
            return Err(());
        }
    }
    Ok(())
}

fn encode_segments(segments: &[&str]) -> Result<String, ()> {
    let mut out = String::new();
    for (index, segment) in segments.iter().enumerate() {
        if index > 0 {
            out.push('/');
        }
        encode_segment(segment, &mut out);
    }
    Ok(out)
}

fn encode_segment(segment: &str, out: &mut String) {
    for byte in segment.bytes() {
        if is_path_char(byte) {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0xf) as usize] as char);
        }
    }
}

fn is_path_char(byte: u8) -> bool {
    matches!(
        byte,
        b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'.'
            | b'_'
            | b'~'
            | b'!'
            | b'$'
            | b'&'
            | b'\''
            | b'('
            | b')'
            | b'*'
            | b'+'
            | b','
            | b';'
            | b'='
            | b':'
            | b'@'
    )
}

/// Converts a `file:` URL path (and optional host) to a filesystem path,
/// matching `Uri.toFilePath` on the current platform.
pub(super) fn uri_to_path(path: &str, host: Option<&str>) -> Option<String> {
    uri_to_path_with(path, host, WINDOWS)
}

pub(super) fn uri_to_path_with(path: &str, host: Option<&str>, windows: bool) -> Option<String> {
    let decoded = percent_decode(path)?;
    if windows {
        if let Some(host) = host {
            if !host.is_empty() {
                return Some(format!(r"\\{}{}", host, decoded.replace('/', "\\")));
            }
        }
        let bytes = decoded.as_bytes();
        if bytes.len() >= 3 && bytes[0] == b'/' && bytes[2] == b':' {
            return Some(decoded[1..].replace('/', "\\"));
        }
        if decoded.starts_with('/') {
            return Some(decoded.replace('/', "\\"));
        }
        Some(decoded.replace('/', "\\"))
    } else {
        Some(decoded)
    }
}

pub(crate) fn decode_relative_path(path: &str) -> String {
    percent_decode(path).unwrap_or_else(|| path.to_string())
}

fn percent_decode(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return None;
            }
            let hi = hex_value(bytes[i + 1])?;
            let lo = hex_value(bytes[i + 2])?;
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[allow(dead_code)]
fn _error() -> DartUriError {
    DartUriError::new("invalid file path")
}
