// dart-source: N/A (end-to-end CLI tests — no Dart counterpart; mirrors the
//   observable behavior of bin/sass.dart and executable/options.dart).

//! End-to-end CLI tests: process exit codes and stdout parity with Dart Sass.

use std::path::PathBuf;
use std::process::Command;

/// The path to the built `rust-sass` binary under test.
fn sass() -> Command {
    Command::new(env!("CARGO_BIN_EXE_rust-sass"))
}

/// Creates a unique scratch directory under the system temp dir.
fn scratch_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rust-sass-cli-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn help_exits_64() {
    let output = sass().arg("--help").output().unwrap();
    assert_eq!(output.status.code(), Some(64));
}

#[test]
fn version_exits_0() {
    let output = sass().arg("--version").output().unwrap();
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn file_compile_prints_no_success_line() {
    let dir = scratch_dir();
    let input = dir.join("input.scss");
    let output = dir.join("output.css");
    std::fs::write(&input, "a { b: c; }").unwrap();

    let result = sass()
        .arg("--no-source-map")
        .arg(&input)
        .arg(&output)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(0));

    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        !stdout.contains("Compiled") && !stderr.contains("Compiled"),
        "CLI must not print a success line; stdout: {stdout:?}, stderr: {stderr:?}"
    );

    let css = std::fs::read_to_string(&output).unwrap();
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(css, "a {\n  b: c;\n}\n");
}
