// Golden-value collector for CompileFailure formatting.
//
// Spawns the Dart embedded compiler (the golden reference), sends a
// VersionRequest + CompileRequest for each error case, and prints the
// CompileFailure fields as Rust literals ready to copy into
// `src/error.rs` unit tests.
//
// Run with:
//   cargo test -p rust-sass-embedded --test collect_golden -- --ignored --nocapture
//
// The compiler defaults to the sass-embedded npm package's bundled Dart
// compiler under rust/rust-sass-wasm/node_modules; override with the
// DART_SASS_EMBEDDED env var (full path to the compiler executable).

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};

use prost::Message;

use rust_sass_embedded::embedded_sass::inbound_message::compile_request;
use rust_sass_embedded::embedded_sass::outbound_message::compile_response;
use rust_sass_embedded::embedded_sass::{
    inbound_message, outbound_message, InboundMessage, OutboundMessage, OutputStyle, Syntax,
};
use rust_sass_embedded::packet::{parse_packet, read_packet, serialize_packet, write_packet};

struct Case {
    name: &'static str,
    source: &'static str,
    url: &'static str,
    color: bool,
    ascii: bool,
}

const CASES: &[Case] = &[
    Case {
        name: "parse_no_url",
        source: "a { color: }",
        url: "",
        color: false,
        ascii: false,
    },
    Case {
        name: "parse_url",
        source: "a { color: }",
        url: "https://example.com/foo.scss",
        color: false,
        ascii: false,
    },
    Case {
        name: "parse_color",
        source: "a { color: }",
        url: "",
        color: true,
        ascii: false,
    },
    Case {
        name: "parse_url_color_ascii",
        source: "a { color: }",
        url: "https://example.com/foo.scss",
        color: true,
        ascii: true,
    },
    Case {
        name: "error_no_url",
        source: "@error \"boom\"",
        url: "",
        color: false,
        ascii: false,
    },
    Case {
        name: "error_url_color",
        source: "@error \"boom\"",
        url: "https://example.com/foo.scss",
        color: true,
        ascii: false,
    },
    Case {
        name: "error_ascii",
        source: "@error \"boom\"",
        url: "",
        color: false,
        ascii: true,
    },
    Case {
        name: "undefined_var_no_url",
        source: "a { b: $nope; }",
        url: "",
        color: false,
        ascii: false,
    },
    Case {
        name: "undefined_var_url",
        source: "a { b: $nope; }",
        url: "https://example.com/foo.scss",
        color: false,
        ascii: false,
    },
    Case {
        name: "unknown_use_no_url",
        source: "@use \"nope\";",
        url: "",
        color: false,
        ascii: false,
    },
    Case {
        name: "extend_media_url_color",
        source: "@media screen { .b { @extend .a; } }",
        url: "https://example.com/foo.scss",
        color: true,
        ascii: false,
    },
];

fn build_compile_request(case: &Case) -> Vec<u8> {
    let req = InboundMessage {
        message: Some(inbound_message::Message::CompileRequest(
            inbound_message::CompileRequest {
                style: OutputStyle::Expanded as i32,
                source_map: false,
                importers: Vec::new(),
                global_functions: Vec::new(),
                alert_color: case.color,
                alert_ascii: case.ascii,
                verbose: false,
                quiet_deps: false,
                source_map_include_sources: false,
                charset: true,
                silent: false,
                fatal_deprecation: Vec::new(),
                silence_deprecation: Vec::new(),
                future_deprecation: Vec::new(),
                input: Some(compile_request::Input::String(
                    compile_request::StringInput {
                        source: case.source.to_string(),
                        url: case.url.to_string(),
                        syntax: Syntax::Scss as i32,
                        importer: None,
                    },
                )),
            },
        )),
    };
    serialize_packet(1, &req)
}

fn build_version_request() -> Vec<u8> {
    let req = InboundMessage {
        message: Some(inbound_message::Message::VersionRequest(
            inbound_message::VersionRequest { id: 0 },
        )),
    };
    serialize_packet(0, &req)
}

fn spawn_compiler() -> Child {
    let (exe, extra) = match std::env::var("DART_SASS_EMBEDDED") {
        Ok(cmd) => (cmd, vec![]),
        Err(_) => {
            let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            let base = root
                .join("rust/rust-sass-wasm/node_modules/sass-embedded-darwin-arm64/dart-sass/src");
            (
                base.join("dart").to_string_lossy().into_owned(),
                vec![base.join("sass.snapshot").to_string_lossy().into_owned()],
            )
        }
    };
    let mut cmd = Command::new(&exe);
    cmd.args(&extra)
        .arg("--embedded")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd.spawn().expect("failed to spawn Dart embedded compiler")
}

fn exchange(compile_packet: &[u8]) -> OutboundMessage {
    let mut child = spawn_compiler();
    let mut stdin = child.stdin.take().unwrap();
    // `serialize_packet` produces the inner [varint id][message bytes]; the
    // wire format adds an outer length varint (write_packet).
    write_packet(&mut stdin, &build_version_request()).unwrap();
    write_packet(&mut stdin, compile_packet).unwrap();
    stdin.flush().unwrap();
    // Keep stdin open: the compiler's dispatcher exits on stdin EOF, which can
    // abort the (asynchronous) compilation before its result is written.
    let _keep_stdin_alive = stdin;

    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let mut responses = Vec::new();
    for _ in 0..2 {
        match read_packet(&mut stdout) {
            Ok(packet) => {
                let (_, buf) = parse_packet(&packet).unwrap();
                responses.push(OutboundMessage::decode(buf).unwrap());
            }
            Err(e) => {
                let mut err = String::new();
                let _ = stderr.read_to_string(&mut err);
                panic!(
                    "failed reading response: {e}; child exited={:?}; stderr={err}",
                    child.try_wait().ok()
                );
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    responses.pop().unwrap()
}

fn print_failure(name: &str, case: &Case, msg: &OutboundMessage) {
    let Some(outbound_message::Message::CompileResponse(resp)) = &msg.message else {
        panic!("expected CompileResponse for {name}, got {:?}", msg.message);
    };
    let Some(compile_response::Result::Failure(failure)) = &resp.result else {
        panic!("expected CompileFailure for {name}");
    };
    println!(
        "// {name} | url={:?} | color={} | ascii={}",
        case.url, case.color, case.ascii
    );
    println!("failure.message => {:?}", failure.message);
    println!("failure.span => {:?}", failure.span);
    println!("failure.stack_trace => {:?}", failure.stack_trace);
    println!("failure.formatted => {:?}", failure.formatted);
    println!();
}

#[test]
#[ignore]
fn collect_dart_goldens() {
    for case in CASES {
        let packet = build_compile_request(case);
        let msg = exchange(&packet);
        print_failure(case.name, case, &msg);
    }
}
