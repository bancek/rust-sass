use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::Ordering;

use rust_sass_spec::RunnerStats;

const DEFAULT_SPEC_PATH: &str = "../sass-spec/spec";

#[test]
#[ignore = "requires sass-spec checkout"]
fn spec_suite() {
    let (spec_path, spec_root) = resolve_paths();

    #[cfg(feature = "leak-gate")]
    leak_gate_run(&spec_path, &spec_root);

    #[cfg(not(feature = "leak-gate"))]
    {
        let (failures, stats) = rust_sass_spec::run_specs(&spec_path, &spec_root);
        report_and_assert(failures, &stats);
    }
}

fn resolve_paths() -> (String, String) {
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

    (spec_path, spec_root)
}

#[derive(Clone, Copy)]
struct Counters {
    total: usize,
    passed: usize,
    warnings_compared: usize,
    errors_compared: usize,
}

fn counters(stats: &RunnerStats) -> Counters {
    Counters {
        total: stats.total.load(Ordering::Relaxed),
        passed: stats.passed.load(Ordering::Relaxed),
        warnings_compared: stats.warnings_compared.load(Ordering::Relaxed),
        errors_compared: stats.errors_compared.load(Ordering::Relaxed),
    }
}

fn stats_line(c: Counters) -> String {
    let pct = if c.total > 0 {
        (c.passed as f64 / c.total as f64) * 100.0
    } else {
        0.0
    };
    format!(
        "\n{}/{} passed ({:.1}%) — {} warnings, {} errors compared",
        c.passed, c.total, pct, c.warnings_compared, c.errors_compared
    )
}

fn assert_corpus(c: Counters) {
    // Only a full-suite run is guaranteed to exercise warning/error files;
    // focused `SASS_SPEC` subsets frequently contain neither.
    if std::env::var_os("SASS_SPEC").is_some() {
        return;
    }
    // The corpus must actually exercise warning and error files; if a future
    // corpus change silently drops them, this catches it.
    assert!(
        c.warnings_compared > 0 && c.errors_compared > 0,
        "spec run compared {} warnings and {} errors; expected both to be non-zero",
        c.warnings_compared,
        c.errors_compared
    );
}

fn report_and_assert(failures: Vec<String>, stats: &RunnerStats) {
    let c = counters(stats);
    let stats_str = stats_line(c);

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

    assert_corpus(c);
}

/// Runs the suite twice: an untracked warm-up (fills process-lifetime caches)
/// and a measured pass. Asserts that the measured pass retains no bytes.
///
/// `LEAK_GATE_ALLOW` sets a byte allowance for a known baseline while the leak
/// fixes land; it defaults to 0, i.e. any retention fails.
#[cfg(feature = "leak-gate")]
fn leak_gate_run(spec_path: &str, spec_root: &str) {
    let (failures, stats) = rust_sass_spec::run_specs(spec_path, spec_root);
    report_and_assert(failures, &stats);
    drop(stats);

    rust_sass_spec::leak_gate::start();
    let (failures, stats) = rust_sass_spec::run_specs(spec_path, spec_root);
    let c = counters(&stats);
    let failure_count = failures.len();
    // Only allocated when the run failed, in which case the test panics and
    // the distorted leak number does not matter.
    let failure_text = if failures.is_empty() {
        String::new()
    } else {
        failures.join("\n\n")
    };
    drop(failures);
    drop(stats);
    rust_sass_spec::leak_gate::stop();

    let leaked = rust_sass_spec::leak_gate::live();
    let mut offenders = rust_sass_spec::leak_gate::report();
    offenders.sort_by_key(|&(_, delta)| std::cmp::Reverse(delta));

    let stats_str = stats_line(c);
    if failure_count == 0 {
        writeln!(std::io::stderr().lock(), "{stats_str}").unwrap();
    } else {
        panic!("{failure_count} spec test(s) failed:\n\n{failure_text}\n\n{stats_str}");
    }
    assert_corpus(c);

    {
        let mut stderr = std::io::stderr().lock();
        let _ = writeln!(
            stderr,
            "\nleak gate: {leaked} bytes retained across the measured pass \
             ({} tests with a non-zero delta)",
            offenders.len()
        );
        for (path, delta) in offenders.iter().take(20) {
            let _ = writeln!(stderr, "  {delta:>12}  {path}");
        }
    }

    let allowed: isize = std::env::var("LEAK_GATE_ALLOW")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    assert!(
        leaked <= allowed,
        "leak gate: {leaked} bytes retained after the measured pass (allowed {allowed})"
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
