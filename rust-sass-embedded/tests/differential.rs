// Differential integration suite: our embedded server (run in-process) vs the
// Dart embedded compiler (the golden reference). Both receive identical
// protocol input and must produce byte-identical stdout.
//
// The Dart compiler path comes from the DART_SASS_EMBEDDED env var, or
// defaults to the sass-embedded npm package's bundled Dart compiler under
// rust/rust-sass-wasm/node_modules. The test skips if Dart isn't available
// (e.g., CI without node_modules).
//
// Cases can script host responses: `build_stdin` appends the deterministic
// `CanonicalizeResponse`/`ImportResponse`/`FileImportResponse`/
// `FunctionCallResponse` packets after the `CompileRequest`, so the suites can
// exercise host-callback flows (relative importers, host functions, arglists,
// calculations) purely through pre-baked stdin. Each case's `expect` is
// asserted against the DART stream only, so a mis-scripted case (one Dart
// itself can't satisfy) is caught as a harness bug rather than a silent pass.

mod common;

use std::io::Cursor;
use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use prost::Message;

use common::{dart_command, normalize};
use rust_sass_embedded::embedded_sass::inbound_message::{
    canonicalize_response, compile_request, file_import_response, function_call_response,
    import_response,
};
use rust_sass_embedded::embedded_sass::outbound_message::compile_response;
use rust_sass_embedded::embedded_sass::value::{
    calculation as proto_calc, calculation::calculation_value,
};
use rust_sass_embedded::embedded_sass::{
    inbound_message, outbound_message, value as proto_value, CalculationOperator, InboundMessage,
    OutboundMessage, OutputStyle, SingletonValue, Syntax,
};
use rust_sass_embedded::packet::{parse_packet, read_packet, serialize_packet, write_packet};

/// What the Dart golden compiler must produce for a case (asserted against the
/// Dart stream only). The ours-vs-Dart byte comparison handles the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaseExpect {
    Success,
    SuccessCss(&'static str),
    Failure,
}

/// A temporary directory holding fixture files (node-package fixtures, path
/// inputs). Kept alive by the `Case` that owns it.
struct Sandbox {
    _dir: tempfile::TempDir,
    root: String,
}

impl Sandbox {
    fn new(files: &[(&str, &str)]) -> Sandbox {
        let dir = tempfile::tempdir().unwrap();
        for (rel, contents) in files {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, contents).unwrap();
        }
        let root = dir.path().to_string_lossy().into_owned();
        Sandbox { _dir: dir, root }
    }

    fn file_url(&self, rel: &str) -> String {
        format!("file://{}/{}", self.root, rel)
    }
}

struct Case {
    name: &'static str,
    source: &'static str,
    url: &'static str,
    color: bool,
    ascii: bool,
    style: OutputStyle,
    verbose: bool,
    silence: &'static [&'static str],
    fatal: &'static [&'static str],
    future: &'static [&'static str],
    global_functions: &'static [&'static str],
    importers: Vec<compile_request::Importer>,
    input_importer: Option<compile_request::Importer>,
    /// Path input (overrides the string input). Resolved against the sandbox.
    path: Option<String>,
    /// Scripted host responses, in the order the compiler requests them.
    responses: Vec<InboundMessage>,
    sandbox: Option<Sandbox>,
    expect: CaseExpect,
}

/// A `Case` with default options (expanded, no color/ascii/deprecations).
fn base(name: &'static str, source: &'static str, url: &'static str) -> Case {
    Case {
        name,
        source,
        url,
        color: false,
        ascii: false,
        style: OutputStyle::Expanded,
        verbose: false,
        silence: &[],
        fatal: &[],
        future: &[],
        global_functions: &[],
        importers: Vec::new(),
        input_importer: None,
        path: None,
        responses: Vec::new(),
        sandbox: None,
        expect: CaseExpect::Success,
    }
}

// ---------- Scripted host responses ----------

fn canonicalize_response(url: &str) -> InboundMessage {
    InboundMessage {
        message: Some(inbound_message::Message::CanonicalizeResponse(
            inbound_message::CanonicalizeResponse {
                id: 0,
                containing_url_unused: true,
                result: Some(canonicalize_response::Result::Url(url.to_string())),
            },
        )),
    }
}

fn import_response(contents: &str) -> InboundMessage {
    InboundMessage {
        message: Some(inbound_message::Message::ImportResponse(
            inbound_message::ImportResponse {
                id: 0,
                result: Some(import_response::Result::Success(
                    import_response::ImportSuccess {
                        contents: contents.to_string(),
                        syntax: Syntax::Scss as i32,
                        source_map_url: None,
                    },
                )),
            },
        )),
    }
}

fn file_import_response(file_url: String) -> InboundMessage {
    InboundMessage {
        message: Some(inbound_message::Message::FileImportResponse(
            inbound_message::FileImportResponse {
                id: 0,
                containing_url_unused: false,
                result: Some(file_import_response::Result::FileUrl(file_url)),
            },
        )),
    }
}

fn function_call_response(value: proto_value::Value) -> InboundMessage {
    InboundMessage {
        message: Some(inbound_message::Message::FunctionCallResponse(
            inbound_message::FunctionCallResponse {
                id: 0,
                accessed_argument_lists: Vec::new(),
                result: Some(function_call_response::Result::Success(
                    rust_sass_embedded::embedded_sass::Value { value: Some(value) },
                )),
            },
        )),
    }
}

// ---------- Calculation proto builders ----------

fn num_arg(v: f64) -> proto_calc::CalculationValue {
    proto_calc::CalculationValue {
        value: Some(calculation_value::Value::Number(proto_value::Number {
            value: v,
            numerators: Vec::new(),
            denominators: Vec::new(),
        })),
    }
}

fn op_arg(
    operator: CalculationOperator,
    left: proto_calc::CalculationValue,
    right: proto_calc::CalculationValue,
) -> proto_calc::CalculationValue {
    proto_calc::CalculationValue {
        value: Some(calculation_value::Value::Operation(Box::new(
            proto_calc::CalculationOperation {
                operator: operator as i32,
                left: Some(Box::new(left)),
                right: Some(Box::new(right)),
            },
        ))),
    }
}

fn calc_arg(name: &str, args: Vec<proto_calc::CalculationValue>) -> proto_calc::CalculationValue {
    proto_calc::CalculationValue {
        value: Some(calculation_value::Value::Calculation(
            proto_value::Calculation {
                name: name.to_string(),
                arguments: args,
            },
        )),
    }
}

fn calc_value(name: &str, args: Vec<proto_calc::CalculationValue>) -> proto_value::Value {
    proto_value::Value::Calculation(proto_value::Calculation {
        name: name.to_string(),
        arguments: args,
    })
}

// ---------- Importer builders ----------

fn host_importer(id: u32, schemes: &[&str]) -> compile_request::Importer {
    compile_request::Importer {
        non_canonical_scheme: schemes.iter().map(|s| s.to_string()).collect(),
        importer: Some(compile_request::importer::Importer::ImporterId(id)),
    }
}

fn file_importer(id: u32) -> compile_request::Importer {
    compile_request::Importer {
        non_canonical_scheme: Vec::new(),
        importer: Some(compile_request::importer::Importer::FileImporterId(id)),
    }
}

fn node_package_importer(entry: &str) -> compile_request::Importer {
    compile_request::Importer {
        non_canonical_scheme: Vec::new(),
        importer: Some(compile_request::importer::Importer::NodePackageImporter(
            rust_sass_embedded::embedded_sass::NodePackageImporter {
                entry_point_directory: entry.to_string(),
            },
        )),
    }
}

// ---------- Cases ----------

fn cases() -> Vec<Case> {
    let mut cases = Vec::new();

    // The original differential matrix (success + error formatting).
    for c in [
        base("success_expanded", "a { b: c }", ""),
        base("success_compressed", "a { b: c }", ""),
        base("parse_no_url", "a { color: }", ""),
        base("parse_url", "a { color: }", "https://example.com/foo.scss"),
        base("parse_color", "a { color: }", ""),
        base(
            "parse_url_color_ascii",
            "a { color: }",
            "https://example.com/foo.scss",
        ),
        base("error_no_url", "@error \"boom\"", ""),
        base(
            "error_url_color",
            "@error \"boom\"",
            "https://example.com/foo.scss",
        ),
        base("error_ascii", "@error \"boom\"", ""),
        base("undefined_var_no_url", "a { b: $nope; }", ""),
        base(
            "undefined_var_url",
            "a { b: $nope; }",
            "https://example.com/foo.scss",
        ),
        base("unknown_use_no_url", "@use \"nope\";", ""),
        base(
            "extend_media_url_color",
            "@media screen { .b { @extend .a; } }",
            "https://example.com/foo.scss",
        ),
    ] {
        cases.push(c);
    }
    let c = &mut cases[1];
    c.style = OutputStyle::Compressed;
    let c = &mut cases[4];
    c.color = true;
    let c = &mut cases[5];
    c.color = true;
    c.ascii = true;
    let c = &mut cases[7];
    c.color = true;
    let c = &mut cases[8];
    c.ascii = true;
    let c = &mut cases[12];
    c.color = true;
    // The original matrix's parse/error/undefined/use/extend cases all expect a
    // `CompileFailure` from the golden compiler.
    for i in [2usize, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12] {
        cases[i].expect = CaseExpect::Failure;
    }

    // ── Cluster A: deprecation processing (silence/fatal/future/limit) ─────
    let mut c = base("deprec_silence", "a { $b: c !global; }", "");
    c.silence = &["new-global"];
    cases.push(c);

    let mut c = base("deprec_fatal", "a { $b: c !global; }", "");
    c.fatal = &["new-global"];
    c.expect = CaseExpect::Failure;
    cases.push(c);

    let mut c = base("deprec_fatal_version", "a { $b: c !global; }", "");
    c.fatal = &["1.17.2"];
    c.expect = CaseExpect::Failure;
    cases.push(c);

    let mut c = base("deprec_future_active", "a { b: c; }", "");
    c.future = &["new-global"];
    cases.push(c);

    let mut c = base("deprec_fatal_silenced", "a { $b: c !global; }", "");
    c.fatal = &["new-global"];
    c.silence = &["new-global"];
    c.expect = CaseExpect::Failure;
    cases.push(c);

    let c = base(
        "deprec_verbose_limit",
        "$_: 1/2; $_: 1/3; $_: 1/4; $_: 1/5; $_: 1/6; $_: 1/7;",
        "",
    );
    cases.push(c);

    let mut c = base("deprec_slash_silence", "$_: 1/2;", "");
    c.silence = &["slash-div"];
    cases.push(c);

    let mut c = base("deprec_slash_fatal", "$_: 1/2;", "");
    c.fatal = &["slash-div"];
    c.expect = CaseExpect::Failure;
    cases.push(c);

    // ── Cluster G: invalid host-function signatures with whitespace ────────
    let mut c = base("sig_ws_before", "", "");
    c.global_functions = &[" foo()"];
    c.expect = CaseExpect::Failure;
    cases.push(c);

    let mut c = base("sig_ws_after", "", "");
    c.global_functions = &["foo() "];
    c.expect = CaseExpect::Failure;
    cases.push(c);

    let mut c = base("sig_ws_between", "", "");
    c.global_functions = &["foo ()"];
    c.expect = CaseExpect::Failure;
    cases.push(c);

    // ── Cluster F: invalid non-canonical scheme ────────────────────────────
    let mut c = base("scheme_uppercase", "a { b: c }", "");
    c.importers = vec![host_importer(1, &["U"])];
    c.expect = CaseExpect::Failure;
    cases.push(c);

    // ── Cluster B: relative-URL resolution against the base importer ───────
    let mut c = base("rel_from_entrypoint", "@use \"orange\";", "u:entrypoint");
    c.input_importer = Some(host_importer(1, &[]));
    c.responses = vec![
        canonicalize_response("u:orange"),
        import_response(".orange {color: orange}"),
    ];
    cases.push(c);

    let mut c = base("rel_no_entrypoint_url", "@use \"orange\";", "");
    c.input_importer = Some(host_importer(1, &[]));
    c.responses = vec![
        canonicalize_response("u:orange"),
        import_response(".orange {color: orange}"),
    ];
    cases.push(c);

    let mut c = base("rel_to_entrypoint_url", "@use \"baz/qux\";", "u:foo/bar");
    c.input_importer = Some(host_importer(1, &[]));
    c.responses = vec![
        canonicalize_response("u:foo/baz/qux"),
        import_response("a {result: x}"),
    ];
    cases.push(c);

    let mut c = base("rel_precedence", "@use \"other\";", "o:style.scss");
    c.importers = vec![host_importer(2, &[])];
    c.input_importer = Some(host_importer(1, &[]));
    c.responses = vec![
        canonicalize_response("o:other"),
        import_response("a {from: relative}"),
    ];
    cases.push(c);

    let sb = Sandbox::new(&[
        (
            "main.scss",
            "@use \"sub1/test\"; @use \"sub1/sub2/test\" as test2;",
        ),
        ("sub1/test.scss", "@use \"y\""),
        ("sub1/x.scss", "x { from: sub1; }"),
        ("sub1/sub2/test.scss", "@use \"y\""),
        ("sub1/sub2/x.scss", "x { from: sub2; }"),
    ]);
    let mut c = base("rel_diff_base", "", "");
    c.path = Some(format!("{}/main.scss", sb.root));
    c.importers = vec![file_importer(1)];
    c.responses = vec![
        file_import_response(sb.file_url("sub1/x.scss")),
        file_import_response(sb.file_url("sub1/sub2/x.scss")),
    ];
    c.sandbox = Some(sb);
    cases.push(c);

    // ── Cluster C: loaded_urls deduplication ───────────────────────────────
    let mut c = base(
        "loaded_urls_dedup",
        "@use \"left\"; @use \"right\";",
        "u:entrypoint",
    );
    c.importers = vec![host_importer(1, &[])];
    c.responses = vec![
        canonicalize_response("u:left"),
        import_response("@use \"upstream\""),
        canonicalize_response("u:upstream"),
        import_response("a { b: c }"),
        canonicalize_response("u:right"),
        import_response("@use \"upstream\""),
    ];
    cases.push(c);

    // ── Cluster D: unused argument-list keywords must carry a span ─────────
    let mut c = base("arglist_unaccessed", "a {b: foo($bar: baz)}", "");
    c.global_functions = &["foo($args...)"];
    c.responses = vec![function_call_response(proto_value::Value::Singleton(
        SingletonValue::Null as i32,
    ))];
    c.expect = CaseExpect::Failure;
    cases.push(c);

    // ── Cluster E: calculation deprotofy must simplify operations ──────────
    let mut c = base("calc_simplify", "a {b: foo()}", "");
    c.global_functions = &["foo()"];
    c.responses = vec![function_call_response(calc_value(
        "calc",
        vec![op_arg(
            CalculationOperator::Plus,
            num_arg(1.0),
            num_arg(2.0),
        )],
    ))];
    c.expect = CaseExpect::SuccessCss("a {\n  b: 3;\n}");
    cases.push(c);

    let mut c = base("calc_operations", "a {b: foo()}", "");
    c.global_functions = &["foo()"];
    let div = op_arg(CalculationOperator::Divide, num_arg(4.0), num_arg(5.0));
    let sub = op_arg(CalculationOperator::Minus, num_arg(3.0), div);
    let mul = op_arg(
        CalculationOperator::Times,
        calc_arg("max", vec![num_arg(5.0), num_arg(6.0)]),
        sub,
    );
    let add = op_arg(
        CalculationOperator::Plus,
        calc_arg("min", vec![num_arg(3.0), num_arg(4.0)]),
        mul,
    );
    c.responses = vec![function_call_response(calc_value("calc", vec![add]))];
    c.expect = CaseExpect::SuccessCss("a {\n  b: 16.2;\n}");
    cases.push(c);

    // ── Cluster I: node-package importer percent-decodes package names ─────
    let sb = Sandbox::new(&[
        (
            "node_modules/foo/package.json",
            r#"{"exports": {".": {"sass": "./src/sass/_sass.scss"}}}"#,
        ),
        (
            "node_modules/foo/src/sass/_sass.scss",
            "a {from: sassCondition}",
        ),
    ]);
    let mut c = base("pkg_percent", "@use \"pkg:%66oo\";", "");
    c.importers = vec![node_package_importer(&sb.root)];
    c.sandbox = Some(sb);
    c.expect = CaseExpect::SuccessCss("a {\n  from: sassCondition;\n}");
    cases.push(c);

    cases
}

fn build_stdin(case: &Case) -> Vec<u8> {
    let mut out = build_stdin_initial(case);
    for resp in &case.responses {
        write_packet(&mut out, &serialize_packet(1, resp)).unwrap();
    }
    out
}

/// The VersionRequest + CompileRequest prefix (without scripted responses).
fn build_stdin_initial(case: &Case) -> Vec<u8> {
    let version = InboundMessage {
        message: Some(inbound_message::Message::VersionRequest(
            inbound_message::VersionRequest { id: 0 },
        )),
    };
    let input = if let Some(path) = &case.path {
        compile_request::Input::Path(path.clone())
    } else {
        compile_request::Input::String(compile_request::StringInput {
            source: case.source.to_string(),
            url: case.url.to_string(),
            syntax: Syntax::Scss as i32,
            importer: case.input_importer.clone(),
        })
    };
    let compile = InboundMessage {
        message: Some(inbound_message::Message::CompileRequest(
            inbound_message::CompileRequest {
                style: case.style as i32,
                source_map: false,
                importers: case.importers.clone(),
                global_functions: case
                    .global_functions
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                alert_color: case.color,
                alert_ascii: case.ascii,
                verbose: case.verbose,
                quiet_deps: false,
                source_map_include_sources: false,
                charset: true,
                silent: false,
                fatal_deprecation: case.fatal.iter().map(|s| s.to_string()).collect(),
                silence_deprecation: case.silence.iter().map(|s| s.to_string()).collect(),
                future_deprecation: case.future.iter().map(|s| s.to_string()).collect(),
                input: Some(input),
            },
        )),
    };
    let mut out = Vec::new();
    write_packet(&mut out, &serialize_packet(0, &version)).unwrap();
    write_packet(&mut out, &serialize_packet(1, &compile)).unwrap();
    out
}

/// A readable one-line description of each decoded outbound message (for
/// diagnostics; not part of the comparison).
fn summarize(stream: &[(u32, Vec<u8>)]) -> Vec<String> {
    stream
        .iter()
        .map(|(id, bytes)| {
            let msg = OutboundMessage::decode(bytes.as_slice()).unwrap();
            let desc = match &msg.message {
                Some(outbound_message::Message::VersionResponse(_)) => "VersionResponse".into(),
                Some(outbound_message::Message::CompileResponse(r)) => match &r.result {
                    Some(compile_response::Result::Success(s)) => format!(
                        "CompileSuccess css={:?} loaded_urls={:?}",
                        s.css, r.loaded_urls
                    ),
                    Some(compile_response::Result::Failure(f)) => format!(
                        "CompileFailure msg={:?} span={:?} formatted={:?}",
                        f.message, f.span, f.formatted
                    ),
                    None => "CompileResponse".into(),
                },
                Some(outbound_message::Message::LogEvent(l)) => {
                    format!("LogEvent type={} msg={:?}", l.r#type, l.message)
                }
                Some(outbound_message::Message::CanonicalizeRequest(r)) => {
                    format!(
                        "CanonicalizeRequest url={:?} importer={}",
                        r.url, r.importer_id
                    )
                }
                Some(outbound_message::Message::ImportRequest(r)) => {
                    format!("ImportRequest url={:?}", r.url)
                }
                Some(outbound_message::Message::FileImportRequest(r)) => {
                    format!(
                        "FileImportRequest url={:?} containing={:?}",
                        r.url, r.containing_url
                    )
                }
                Some(outbound_message::Message::FunctionCallRequest(r)) => {
                    format!("FunctionCallRequest args={}", r.arguments.len())
                }
                Some(outbound_message::Message::Error(e)) => format!("Error {}", e.message),
                _ => "?".into(),
            };
            format!("[{id}] {desc}")
        })
        .collect()
}

/// Asserts that the DART stream ends with a `CompileResponse` matching `expect`.
/// This is the "must pass with Dart" guard: a case Dart itself can't satisfy is
/// a harness bug, not a compiler divergence.
fn check_dart_expect(dart_out: &[(u32, Vec<u8>)], expect: CaseExpect) -> Result<(), String> {
    let Some((id, bytes)) = dart_out.last() else {
        return Err("Dart produced no output".into());
    };
    if *id != 1 {
        return Err(format!(
            "Dart's final packet had compilation ID {id}, not 1"
        ));
    }
    let msg = OutboundMessage::decode(bytes.as_slice()).unwrap();
    let Some(outbound_message::Message::CompileResponse(resp)) = &msg.message else {
        return Err("Dart's final packet was not a CompileResponse".into());
    };
    match (expect, &resp.result) {
        (CaseExpect::Success, Some(compile_response::Result::Success(_))) => Ok(()),
        (CaseExpect::SuccessCss(css), Some(compile_response::Result::Success(s)))
            if s.css == css =>
        {
            Ok(())
        }
        (CaseExpect::Failure, Some(compile_response::Result::Failure(_))) => Ok(()),
        (expected, actual) => Err(format!(
            "expected {expected:?} but final CompileResponse was {actual:?}"
        )),
    }
}

/// Runs our server in-process and returns the normalized packets: `(compilation_id,
/// canonical prost re-encoding of the decoded message)`, plus the process exit
/// code. Re-encoding with prost normalizes away Dart's protobuf-library presence
/// quirks (Dart emits zero-valued proto3 scalars like `line: 0` that prost
/// omits), so identical decoded messages yield identical bytes.
///
/// The exit code is returned (not asserted) because a buggy server may diverge
/// in a way that surfaces as a protocol error; the stream comparison reports it.
fn run_ours(stdin: &[u8]) -> (Vec<(u32, Vec<u8>)>, i32) {
    struct BufWriter(Arc<Mutex<Vec<u8>>>);
    impl Write for BufWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let reader: Box<dyn Read + Send> = Box::new(Cursor::new(stdin.to_vec()));
    let buf = Arc::new(Mutex::new(Vec::new()));
    let writer: rust_sass_embedded::packet::SharedWriter = Arc::new(Mutex::new(
        Box::new(BufWriter(buf.clone())) as Box<dyn Write + Send>,
    ));
    let stderr: rust_sass_embedded::packet::SharedWriter =
        Arc::new(Mutex::new(Box::new(Vec::new()) as Box<dyn Write + Send>));
    let dispatcher = rust_sass_embedded::Dispatcher::new(reader, writer, stderr);
    let code = dispatcher.listen();

    let writer = buf.lock().unwrap().clone();
    let mut out = Vec::new();
    let mut cursor = Cursor::new(writer);
    while let Ok(packet) = read_packet(&mut cursor) {
        let (id, buf) = parse_packet(&packet).unwrap();
        let msg = OutboundMessage::decode(buf).unwrap();
        out.push(normalize(id, &msg));
    }
    (out, code)
}

/// Spawns Dart, writes the `CompileRequest` prefix, then drives it interactively:
/// each host-callback request it emits is answered with the next scripted
/// `response` (Dart's `ReusableIsolate` rejects responses that arrive before
/// its matching request, so pre-baked stdin can't be used). Returns the
/// normalized outbound packets up to and including the terminal
/// `CompileResponse`.
fn run_dart(
    command: &[String],
    initial: &[u8],
    responses: &[InboundMessage],
) -> Vec<(u32, Vec<u8>)> {
    let mut child: Child = Command::new(&command[0])
        .args(&command[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn Dart embedded compiler");
    let mut stdin_h = child.stdin.take().unwrap();
    stdin_h.write_all(initial).unwrap();
    stdin_h.flush().unwrap();

    let mut stdout_h = child.stdout.take().unwrap();
    let mut stderr_h = child.stderr.take().unwrap();
    let mut out = Vec::new();
    let mut next_response = 0usize;
    let result = (|| {
        for _ in 0..100 {
            let packet = match read_packet(&mut stdout_h) {
                Ok(p) => p,
                Err(e) => {
                    let _ = child.kill();
                    let mut err = String::new();
                    let _ = stderr_h.read_to_string(&mut err);
                    panic!("failed reading Dart response packet: {e}; dart stderr: {err}");
                }
            };
            let (id, buf) = parse_packet(&packet).unwrap();
            let msg = OutboundMessage::decode(buf).unwrap();
            let is_final = matches!(
                msg.message,
                Some(outbound_message::Message::CompileResponse(_))
            );
            out.push(normalize(id, &msg));
            if is_final {
                return Ok(());
            }
            match &msg.message {
                Some(
                    outbound_message::Message::CanonicalizeRequest(_)
                    | outbound_message::Message::ImportRequest(_)
                    | outbound_message::Message::FileImportRequest(_)
                    | outbound_message::Message::FunctionCallRequest(_),
                ) => {
                    let resp = responses.get(next_response).unwrap_or_else(|| {
                        panic!(
                            "no scripted response left for {msg:?}; scripted {} responses",
                            responses.len()
                        )
                    });
                    next_response += 1;
                    let bytes = serialize_packet(1, resp);
                    write_packet(&mut stdin_h, &bytes).unwrap();
                    stdin_h.flush().unwrap();
                }
                Some(outbound_message::Message::LogEvent(_)) => {}
                Some(outbound_message::Message::VersionResponse(_)) => {}
                Some(outbound_message::Message::Error(pe)) => {
                    panic!("Dart sent a protocol error: {pe:?}")
                }
                _ => panic!("unexpected outbound message {msg:?}"),
            }
        }
        Err("Dart produced more than 100 packets without a CompileResponse".to_string())
    })();
    let _ = child.kill();
    let _ = child.wait();
    if let Err(e) = result {
        panic!("{e}");
    }
    if next_response != responses.len() {
        panic!(
            "scripted {} responses but Dart consumed {}",
            responses.len(),
            next_response
        );
    }
    out
}

#[test]
fn matches_dart_for_all_inputs() {
    let dart = match dart_command() {
        Some(cmd) => cmd,
        None => {
            eprintln!("Dart embedded compiler not found; skipping differential test");
            return;
        }
    };

    let cases = cases();
    let mut failures = Vec::new();
    for case in &cases {
        let stdin = build_stdin(case);
        let dart_out = run_dart(&dart, &build_stdin_initial(case), &case.responses);
        if let Err(msg) = check_dart_expect(&dart_out, case.expect) {
            failures.push(format!(
                "{}: Dart did not produce the expected output: {msg}",
                case.name
            ));
            continue;
        }
        let (our_out, our_code) = run_ours(&stdin);
        if our_out != dart_out {
            failures.push(format!(
                "{}: mismatch (ours exit code {our_code})\n  ours: {:?}\n  dart: {:?}",
                case.name,
                summarize(&our_out),
                summarize(&dart_out),
            ));
        }
    }
    if !failures.is_empty() {
        panic!(
            "{} differential mismatches:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}
