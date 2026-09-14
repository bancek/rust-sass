// Copyright 2019 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/embedded/utils.dart + lib/src/embedded/util/length_delimited_transformer.dart
//              + lib/src/embedded/util/varint_builder.dart
// go-source: go/embedded/utils.go + go/embedded/util/length_delimited_transformer.go
//              + go/embedded/util/varint_builder.go

use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex};

use prost::Message;

use crate::embedded_sass::ProtocolError;

use crate::error::parse_error;

/// A thread-safe, shared outbound writer (one packet per lock acquisition).
/// Mirrors Go's `concurrentPacketWriter`.
pub type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

/// Encodes `value` as an unsigned varint.
///
/// Matches Go: `EncodeVarint` in util/length_delimited_transformer.go.
pub fn encode_varint(mut v: u32) -> Vec<u8> {
    let mut out = Vec::new();
    if v == 0 {
        return vec![0];
    }
    while v > 0x7f {
        out.push((v as u8 & 0x7f) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
    out
}

/// Builds up an unsigned varint byte-by-byte.
///
/// Matches Go: `VarintBuilder` in util/varint_builder.go.
pub struct VarintBuilder {
    max_length: usize,
    name: String,
    value: u64,
    bits: usize,
    done: bool,
}

impl VarintBuilder {
    /// `max_length` is the maximum number of bits allowed for the integer;
    /// `name` is used in error reporting.
    pub fn new(max_length: usize, name: &str) -> Self {
        VarintBuilder {
            max_length,
            name: name.into(),
            value: 0,
            bits: 0,
            done: false,
        }
    }

    /// Parses `b` as a continuation of the varint.
    ///
    /// Returns `Ok(Some(value))` if this byte completes the varint, `Ok(None)`
    /// if more bytes are needed, and `Err` if the byte causes the length to
    /// exceed `max_length` or if `add` already returned a value.
    pub fn add(&mut self, b: u8) -> Result<Option<u64>, String> {
        if self.done {
            return Err("VarintBuilder.add() has already returned a value.".into());
        }

        self.value |= u64::from(b & 0x7f) << self.bits;
        self.bits += 7;

        if b > 0x7f {
            if self.bits >= self.max_length {
                self.done = true;
                return Err(format!(
                    "varint {}was longer than {} bits",
                    self.name_msg(),
                    self.max_length
                ));
            }
            return Ok(None);
        }

        self.done = true;
        if self.bits > self.max_length && self.value >= (1u64 << self.max_length) {
            return Err(format!(
                "varint {}was longer than {} bits",
                self.name_msg(),
                self.max_length
            ));
        }
        Ok(Some(self.value))
    }

    fn name_msg(&self) -> String {
        if self.name.is_empty() {
            String::new()
        } else {
            format!("{} ", self.name)
        }
    }
}

/// Splits `packet` into `(compilation_id, message_bytes)`.
///
/// Matches Go: `parsePacket`. On error, returns a PARSE `ProtocolError`.
pub fn parse_packet(packet: &[u8]) -> Result<(u32, &[u8]), ProtocolError> {
    let mut builder = VarintBuilder::new(32, "compilation ID");
    for (i, &b) in packet.iter().enumerate() {
        match builder.add(b) {
            Ok(Some(id)) => return Ok((id as u32, &packet[i + 1..])),
            Ok(None) => continue,
            Err(e) => return Err(parse_error(&format!("Invalid compilation ID: {e}"))),
        }
    }
    Err(parse_error(
        "Invalid compilation ID: continuation bit always set.",
    ))
}

/// Builds `packet = [varint compilation_id][message bytes]`.
///
/// Matches Go: `serializePacket`.
pub fn serialize_packet(compilation_id: u32, message: &impl Message) -> Vec<u8> {
    let mut packet = encode_varint(compilation_id);
    packet.extend_from_slice(&message.encode_to_vec());
    packet
}

/// Reads one length-delimited packet from `r`.
///
/// The packet is prefixed with an unsigned varint (max 53 bits) indicating the
/// length of the message. Returns `Err(UnexpectedEof)` on a clean end of
/// stream.
///
/// Matches Go: `ReadPacket` in util/length_delimited_transformer.go.
pub fn read_packet(r: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut builder = VarintBuilder::new(53, "packet length");
    let len = loop {
        let mut b = [0u8; 1];
        r.read_exact(&mut b)?;
        match builder.add(b[0]) {
            Ok(Some(l)) => break l as usize,
            Ok(None) => continue,
            Err(e) => return Err(io::Error::new(io::ErrorKind::InvalidData, e)),
        }
    };
    let mut packet = vec![0u8; len];
    r.read_exact(&mut packet)?;
    Ok(packet)
}

/// Writes one length-delimited packet to `w`, prefixed with an unsigned varint
/// indicating its length.
///
/// Matches Go: `WritePacket` in util/length_delimited_transformer.go.
pub fn write_packet(w: &mut impl Write, packet: &[u8]) -> io::Result<()> {
    let length_varint = encode_varint(packet.len() as u32);
    w.write_all(&length_varint)?;
    w.write_all(packet)
}

/// Serializes `message` as a packet for `compilation_id` and writes it to `w`.
///
/// `w` is generic so tests can use an in-memory buffer; the dispatcher passes
/// `&mut std::io::stdout().lock()` (the process-global stdout lock serializes
/// concurrent writes across threads).
pub fn write_message<W: Write>(
    w: &mut W,
    compilation_id: u32,
    message: &impl Message,
) -> io::Result<()> {
    let packet = serialize_packet(compilation_id, message);
    write_packet(w, &packet)?;
    w.flush()
}

/// Serializes `message` as a packet for `compilation_id` and writes it through
/// the shared writer, locking it for the duration of one full packet.
pub fn write_message_shared(
    writer: &SharedWriter,
    compilation_id: u32,
    message: &impl Message,
) -> io::Result<()> {
    let packet = serialize_packet(compilation_id, message);
    let mut w = writer.lock().unwrap();
    write_packet(&mut *w, &packet)?;
    w.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedded_sass::inbound_message::VersionRequest;
    use crate::embedded_sass::InboundMessage;
    use prost::Message;
    use std::io::Cursor;

    #[test]
    fn encode_varint_small_values() {
        assert_eq!(encode_varint(0), vec![0]);
        assert_eq!(encode_varint(1), vec![1]);
        assert_eq!(encode_varint(127), vec![0x7f]);
        assert_eq!(encode_varint(128), vec![0x80, 0x01]);
        assert_eq!(encode_varint(300), vec![0xac, 0x02]);
    }

    #[test]
    fn encode_varint_max() {
        assert_eq!(encode_varint(u32::MAX), vec![0xff, 0xff, 0xff, 0xff, 0x0f]);
    }

    #[test]
    fn varint_builder_roundtrip() {
        for value in [0u32, 1, 127, 128, 300, u32::MAX] {
            let mut builder = VarintBuilder::new(32, "compilation ID");
            let mut decoded = None;
            for &b in &encode_varint(value) {
                decoded = builder.add(b).unwrap();
                if decoded.is_some() {
                    break;
                }
            }
            assert_eq!(decoded, Some(u64::from(value)));
        }
    }

    #[test]
    fn varint_builder_too_long() {
        let mut builder = VarintBuilder::new(7, "small");
        // A continuation byte past the max bits must error.
        let err = builder.add(0x80).unwrap_err();
        assert!(err.contains("was longer than 7 bits"), "{err}");
    }

    #[test]
    fn varint_builder_reuse_after_done_errors() {
        let mut builder = VarintBuilder::new(32, "compilation ID");
        assert_eq!(builder.add(1).unwrap(), Some(1));
        assert!(builder.add(1).is_err());
    }

    #[test]
    fn parse_packet_roundtrip() {
        let msg = InboundMessage {
            message: Some(
                crate::embedded_sass::inbound_message::Message::VersionRequest(VersionRequest {
                    id: 5,
                }),
            ),
        };
        let packet = serialize_packet(42, &msg);
        let (id, message_bytes) = parse_packet(&packet).unwrap();
        assert_eq!(id, 42);
        let decoded = InboundMessage::decode(message_bytes).unwrap();
        let decoded = decoded.message.unwrap();
        assert!(matches!(
            decoded,
            crate::embedded_sass::inbound_message::Message::VersionRequest(r) if r.id == 5
        ));
    }

    #[test]
    fn parse_packet_continuation_always_set() {
        let err = parse_packet(&[0x80]).unwrap_err();
        assert_eq!(
            err.message,
            "Invalid compilation ID: continuation bit always set."
        );
    }

    #[test]
    fn parse_packet_bad_varint() {
        // 5 continuation bytes = 35 bits, over the 32-bit max for the ID.
        let err = parse_packet(&[0x80, 0x80, 0x80, 0x80, 0x80]).unwrap_err();
        assert!(err.message.starts_with("Invalid compilation ID: "));
    }

    #[test]
    fn read_write_packet_roundtrip() {
        let mut buf = Vec::new();
        write_packet(&mut buf, b"hello").unwrap();
        write_packet(&mut buf, b"world!").unwrap();

        let mut cursor = Cursor::new(buf);
        assert_eq!(read_packet(&mut cursor).unwrap(), b"hello");
        assert_eq!(read_packet(&mut cursor).unwrap(), b"world!");
        assert!(matches!(
            read_packet(&mut cursor).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        ));
    }

    #[test]
    fn read_packet_eof() {
        let mut cursor = Cursor::new(Vec::new());
        assert!(matches!(
            read_packet(&mut cursor).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        ));
    }

    #[test]
    fn write_message_roundtrip() {
        let mut buf = Vec::new();
        let msg = InboundMessage {
            message: Some(
                crate::embedded_sass::inbound_message::Message::VersionRequest(VersionRequest {
                    id: 7,
                }),
            ),
        };
        write_message(&mut buf, 3, &msg).unwrap();

        // Read the framing back: [varint len][varint id][message bytes].
        let mut cursor = Cursor::new(buf);
        let packet = read_packet(&mut cursor).unwrap();
        let (id, message_bytes) = parse_packet(&packet).unwrap();
        assert_eq!(id, 3);
        let decoded = InboundMessage::decode(message_bytes)
            .unwrap()
            .message
            .unwrap();
        assert!(matches!(
            decoded,
            crate::embedded_sass::inbound_message::Message::VersionRequest(r) if r.id == 7
        ));
    }
}
