use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::Ordering;

const DEFAULT_SPEC_PATH: &str = "../sass-spec/spec";

#[test]
#[ignore = "requires sass-spec checkout"]
fn spec_suite() {
    let spec_path = std::env::var("SASS_SPEC_PATH").unwrap_or_else(|_| {
        if let Ok(sub) = std::env::var("SASS_SPEC") {
            let mut p = PathBuf::from(DEFAULT_SPEC_PATH);
            p.push(&sub);
            p.to_string_lossy().to_string()
        } else {
            DEFAULT_SPEC_PATH.to_string()
        }
    });

    let spec_root = if std::env::var("SASS_SPEC_PATH").is_ok() {
        find_spec_root(&spec_path)
    } else {
        DEFAULT_SPEC_PATH.to_string()
    };

    // When both SASS_SPEC_PATH and SASS_SPEC are set, append SASS_SPEC
    // as a sub-path under the resolved spec root.
    let spec_path = if std::env::var("SASS_SPEC_PATH").is_ok() && std::env::var("SASS_SPEC").is_ok()
    {
        let sub = std::env::var("SASS_SPEC").unwrap();
        let mut p = PathBuf::from(&spec_path);
        p.push(&sub);
        p.to_string_lossy().to_string()
    } else {
        spec_path
    };

    let (failures, stats) = rust_sass_spec::run_specs(&spec_path, &spec_root);

    let total = stats.total.load(Ordering::Relaxed);
    let passed = stats.passed.load(Ordering::Relaxed);
    let warnings_compared = stats.warnings_compared.load(Ordering::Relaxed);
    let errors_compared = stats.errors_compared.load(Ordering::Relaxed);
    let pct = if total > 0 {
        (passed as f64 / total as f64) * 100.0
    } else {
        0.0
    };

    let stats_str = format!(
        "\n{}/{} passed ({:.1}%) — {} warnings, {} errors compared",
        passed, total, pct, warnings_compared, errors_compared
    );

    if failures.is_empty() {
        writeln!(std::io::stderr().lock(), "{stats_str}").unwrap();
    } else {
        panic!(
            "{} spec test(s) failed:\n\n{}\n\n{}",
            failures.len(),
            failures.join("\n\n"),
            stats_str
        );
    }

    // The corpus must actually exercise warning and error files; if a future
    // corpus change silently drops them, this catches it.
    assert!(
        warnings_compared > 0 && errors_compared > 0,
        "spec run compared {warnings_compared} warnings and {errors_compared} errors; \
         expected both to be non-zero"
    );
}

fn find_spec_root(spec_path: &str) -> String {
    let abs = std::fs::canonicalize(spec_path).unwrap_or_else(|_| PathBuf::from(spec_path));
    let mut abs = if abs.to_string_lossy().ends_with(".hrx") {
        abs.parent().unwrap().to_path_buf()
    } else {
        abs
    };
    loop {
        if abs.file_name().is_some_and(|n| n == "spec") {
            return abs.to_string_lossy().to_string();
        }
        let parent_str = abs
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let abs_str = abs.to_string_lossy().to_string();
        if parent_str == abs_str || parent_str.is_empty() {
            break;
        }
        abs = abs.parent().unwrap().to_path_buf();
    }
    spec_path.to_string()
}
