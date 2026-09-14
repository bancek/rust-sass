// dart-source: N/A (Go spec runner only)
// go-source: go/spec/runner.go

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::io::Error;
use std::io::ErrorKind;
use std::io::Write;
use std::panic::AssertUnwindSafe;
use std::path::Component;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use bumpalo::Bump;
use regex::Regex;

use rust_sass::common::exception::Trace;
use rust_sass::common::span::Span;
use rust_sass::compile::{compile_stylesheet, CompileOptions};
use rust_sass::deprecation::Deprecation;
use rust_sass::io::{DefaultIo, Io, IoExt, VirtualIo};
use rust_sass::logger::{Logger, StderrLogger};

use crate::hrx::parse_hrx;
use crate::options::{parse_options, SpecOptions};

pub const IMPL_NAME: &str = "dart-sass-rust";
pub const IMPL_NAMES: &[&str] = &["dart-sass-rust", "dart-sass"];

// Matches the reference harness's normalizeOutput path strip
// (sass-spec/lib/test-case/compare.ts:13).
static RE_INPUT_PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[-_/a-zA-Z0-9]+(input\.s[ca]ss)").unwrap());
static RE_CONSECUTIVE_NEWLINES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n+").unwrap());

pub struct RunnerStats {
    pub total: AtomicUsize,
    pub passed: AtomicUsize,
    /// How many tests compared a `warning` file against the compiled warnings.
    pub warnings_compared: AtomicUsize,
    /// How many tests compared an `error` file against the compiled error.
    pub errors_compared: AtomicUsize,
}

impl RunnerStats {
    pub fn new() -> Arc<Self> {
        Arc::new(RunnerStats {
            total: AtomicUsize::new(0),
            passed: AtomicUsize::new(0),
            warnings_compared: AtomicUsize::new(0),
            errors_compared: AtomicUsize::new(0),
        })
    }

    fn inc_warnings_compared(&self) {
        self.warnings_compared.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_errors_compared(&self) {
        self.errors_compared.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_total(&self) -> usize {
        self.total.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn inc_passed(&self) {
        self.passed.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn run_specs(spec_path: &str, spec_root: &str) -> (Vec<String>, Arc<RunnerStats>) {
    let mut failures = Vec::new();
    let stats = RunnerStats::new();

    let abs_path = match absolute_path(spec_path) {
        Ok(p) => p,
        Err(e) => {
            failures.push(format!("resolving spec path {}: {}", spec_path, e));
            return (failures, stats);
        }
    };
    let abs_spec_root = match absolute_path(spec_root) {
        Ok(p) => p,
        Err(e) => {
            failures.push(format!("resolving spec root {}: {}", spec_root, e));
            return (failures, stats);
        }
    };

    let metadata = match std::fs::metadata(&abs_path) {
        Ok(m) => m,
        Err(e) => {
            failures.push(format!("spec path does not exist: {}: {}", abs_path, e));
            return (failures, stats);
        }
    };

    if metadata.is_dir() {
        walk_spec_dir(
            &mut failures,
            &abs_path,
            &abs_path,
            &abs_spec_root,
            &SpecOptions::default(),
            &stats,
        );
    } else if abs_path.ends_with(".hrx") {
        let root = Path::new(&abs_path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| abs_path.clone());
        run_hrx_file(
            &mut failures,
            &abs_path,
            &root,
            &abs_spec_root,
            &SpecOptions::default(),
            &stats,
        );
    } else {
        failures.push(format!(
            "spec path is not a directory or .hrx file: {}",
            abs_path
        ));
    }

    (failures, stats)
}

fn absolute_path(path: &str) -> Result<String, Error> {
    let p = Path::new(path);
    if p.is_absolute() {
        Ok(clean_path_str(path))
    } else {
        let cwd = std::env::current_dir()?;
        Ok(clean_path_str(&cwd.join(p).to_string_lossy()))
    }
}

fn clean_path_str(p: &str) -> String {
    let path = Path::new(p);
    let mut components: Vec<&str> = Vec::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                components.pop();
            }
            Component::Normal(s) => {
                if let Some(s) = s.to_str() {
                    components.push(s);
                }
            }
            _ => {}
        }
    }
    let mut result = String::new();
    if p.starts_with('/') {
        result.push('/');
    }
    for (i, c) in components.iter().enumerate() {
        if i > 0 {
            result.push('/');
        }
        result.push_str(c);
    }
    result
}

fn walk_spec_dir(
    failures: &mut Vec<String>,
    root: &str,
    current_dir: &str,
    spec_root: &str,
    parent_opts: &SpecOptions,
    stats: &Arc<RunnerStats>,
) {
    let dir_opts = match load_dir_options(current_dir, parent_opts) {
        Ok(o) => o,
        Err(e) => {
            failures.push(format!("loading options from {}: {}", current_dir, e));
            return;
        }
    };

    let entries = match std::fs::read_dir(current_dir) {
        Ok(e) => e,
        Err(e) => {
            failures.push(format!("reading dir {}: {}", current_dir, e));
            return;
        }
    };

    let mut hrx_files: Vec<String> = Vec::new();
    let mut subdirs: Vec<String> = Vec::new();
    let mut has_input_file = false;

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if file_type.is_dir() {
            subdirs.push(name);
        } else if name.ends_with(".hrx") {
            hrx_files.push(name);
        } else if name == "input.scss" || name == "input.sass" {
            has_input_file = true;
        }
    }

    hrx_files.sort();
    subdirs.sort();

    if has_input_file {
        let input_file = if std::fs::metadata(format!("{}/input.scss", current_dir)).is_ok() {
            "input.scss"
        } else {
            "input.sass"
        };
        let files = read_physical_dir(current_dir);
        let abs_input = format!("{}/{}", current_dir, input_file);
        run_spec_test(
            failures,
            root,
            current_dir,
            &abs_input,
            input_file,
            &files,
            spec_root,
            &dir_opts,
            stats,
        );
    }

    for hrx_file in &hrx_files {
        let full_path = format!("{}/{}", current_dir, hrx_file);
        run_hrx_file(failures, &full_path, root, spec_root, &dir_opts, stats);
    }

    for subdir in &subdirs {
        let full_path = format!("{}/{}", current_dir, subdir);
        walk_spec_dir(failures, root, &full_path, spec_root, &dir_opts, stats);
    }
}

fn read_physical_dir(dir: &str) -> HashMap<String, String> {
    let mut files = HashMap::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return files,
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if let Ok(ft) = entry.file_type() {
            if ft.is_dir() {
                continue;
            }
        }
        let path = format!("{}/{}", dir, name);
        if let Ok(content) = std::fs::read_to_string(&path) {
            files.insert(path, content);
        }
    }
    files
}

fn load_dir_options(dir: &str, parent: &SpecOptions) -> Result<SpecOptions, String> {
    let opts_path = format!("{}/options.yml", dir);
    let content = match std::fs::read_to_string(&opts_path) {
        Ok(c) => c,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Ok(parent.clone());
        }
        Err(e) => return Err(e.to_string()),
    };
    let child = parse_options(&content)?;
    if parent.precision == 0
        && parent.todo.is_empty()
        && parent.warning_todo.is_empty()
        && parent.ignore_for.is_empty()
    {
        Ok(child)
    } else {
        Ok(parent.merge(&child))
    }
}

fn run_hrx_file(
    failures: &mut Vec<String>,
    hrx_path: &str,
    root: &str,
    spec_root: &str,
    parent_opts: &SpecOptions,
    stats: &Arc<RunnerStats>,
) {
    let content = match std::fs::read_to_string(hrx_path) {
        Ok(c) => c,
        Err(e) => {
            failures.push(format!("reading HRX {}: {}", hrx_path, e));
            return;
        }
    };

    let dir = Path::new(hrx_path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let archive_name = Path::new(hrx_path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    let mut opts = match load_dir_options(&dir, parent_opts) {
        Ok(o) => o,
        Err(e) => {
            failures.push(format!("loading options from {}: {}", dir, e));
            parent_opts.clone()
        }
    };

    let archive = match parse_hrx(&content) {
        Ok(a) => a,
        Err(e) => {
            failures.push(format!("parsing HRX {}: {}", hrx_path, e));
            return;
        }
    };

    if let Some(opts_yaml) = archive.get_options_yaml() {
        if !opts_yaml.trim().is_empty() {
            match parse_options(opts_yaml) {
                Ok(child) => opts = opts.merge(&child),
                Err(e) => failures.push(format!("parsing options.yml in {}: {}", hrx_path, e)),
            }
        }
    }

    let hrx_root = format!("{}/{}", dir, archive_name);

    for test_archive in archive.list_test_dirs() {
        let virt_dir = if test_archive.path.is_empty() {
            hrx_root.clone()
        } else {
            format!("{}/{}", hrx_root, test_archive.path)
        };
        let input_file = test_archive.input_file().to_string();
        let abs_input = format!("{}/{}", virt_dir, input_file);

        let mut files: HashMap<String, String> = HashMap::new();
        for (p, c) in test_archive.all_files() {
            files.insert(format!("{}/{}", virt_dir, p), c);
        }
        for (p, c) in archive.all_files() {
            let abs_path = format!("{}/{}", hrx_root, p);
            files.entry(abs_path).or_insert(c);
        }

        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue;
                }
                if let Ok(ft) = entry.file_type() {
                    if ft.is_dir() {
                        continue;
                    }
                }
                let real_path = format!("{}/{}", dir, name);
                if let Entry::Vacant(e) = files.entry(real_path) {
                    if let Ok(file_content) = std::fs::read_to_string(e.key()) {
                        e.insert(file_content);
                    }
                }
            }
        }

        let mut test_opts = opts.clone();
        if let Some(opts_yaml) = test_archive.get_options_yaml() {
            if !opts_yaml.trim().is_empty() {
                match parse_options(opts_yaml) {
                    Ok(child) => test_opts = test_opts.merge(&child),
                    Err(e) => failures.push(format!(
                        "parsing options.yml in {}/{}: {}",
                        archive_name, test_archive.path, e
                    )),
                }
            }
        }

        run_spec_test(
            failures,
            root,
            &virt_dir,
            &abs_input,
            &input_file,
            &files,
            spec_root,
            &test_opts,
            stats,
        );
    }
}

pub struct TestLogger {
    stderr: StderrLogger,
    buffer: Arc<Mutex<String>>,
}

impl Default for TestLogger {
    fn default() -> Self {
        Self::new()
    }
}

impl TestLogger {
    pub fn new() -> Self {
        let buffer = Arc::new(Mutex::new(String::new()));
        let stderr = StderrLogger::new_with_write_fn(false, false, Rc::new(DefaultIo::new()), {
            let buf = Arc::clone(&buffer);
            move |msg: String| {
                buf.lock().unwrap().push_str(&msg);
            }
        });
        TestLogger { stderr, buffer }
    }

    /// Creates a logger whose error-path rendering uses [io] (so URLs are
    /// humanized relative to the test directory, matching the reference
    /// harness's `cwd: <test-dir>`).
    pub fn new_with_io(io: Rc<dyn Io>) -> Self {
        let buffer = Arc::new(Mutex::new(String::new()));
        let stderr = StderrLogger::new_with_write_fn(false, false, io, {
            let buf = Arc::clone(&buffer);
            move |msg: String| {
                buf.lock().unwrap().push_str(&msg);
            }
        });
        TestLogger { stderr, buffer }
    }

    pub fn output(&self) -> String {
        self.buffer.lock().unwrap().clone()
    }
}

impl Debug for TestLogger {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestLogger").finish()
    }
}

impl Logger for TestLogger {
    fn warn<'a>(&self, message: &str, span: Option<&Span<'a>>, trace: Option<&Trace>) {
        self.stderr.warn(message, span, trace);
    }

    fn debug<'a>(&self, message: &str, span: Option<&Span<'a>>) {
        self.stderr.debug(message, span);
    }

    fn warn_deprecation<'a>(
        &self,
        message: &str,
        span: Option<&Span<'a>>,
        deprecation: &'static Deprecation,
        trace: Option<&Trace>,
    ) -> rust_sass::common::exception::SassResult<()> {
        self.stderr
            .warn_deprecation(message, span, deprecation, trace)
    }
}

// Spec-harness plumbing; arity follows the test-context fields, a params
// struct would just rename them.
#[allow(clippy::too_many_arguments)]
fn run_spec_test(
    failures: &mut Vec<String>,
    root: &str,
    test_dir: &str,
    abs_input: &str,
    _input_file: &str,
    files: &HashMap<String, String>,
    spec_root: &str,
    opts: &SpecOptions,
    stats: &Arc<RunnerStats>,
) {
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    run_spec_test_wrapped(
        failures,
        root,
        test_dir,
        abs_input,
        _input_file,
        files,
        spec_root,
        opts,
        stats,
    );
    std::panic::set_hook(prev_hook);
}

// Same harness-plumbing rationale as `run_spec_test`.
#[allow(clippy::too_many_arguments)]
fn run_spec_test_wrapped(
    failures: &mut Vec<String>,
    root: &str,
    test_dir: &str,
    abs_input: &str,
    _input_file: &str,
    files: &HashMap<String, String>,
    spec_root: &str,
    opts: &SpecOptions,
    stats: &Arc<RunnerStats>,
) {
    let rel_path = Path::new(test_dir)
        .strip_prefix(root)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| test_dir.to_string());

    let n = stats.inc_total();

    let _ = writeln!(std::io::stderr().lock(), "[{}] {} ...", n + 1, rel_path);

    if opts.is_ignored_any(IMPL_NAMES) {
        let _ = writeln!(std::io::stderr().lock(), "[{}] {} ... ignored", n, rel_path);
        return;
    }
    if opts.is_todo_any(IMPL_NAMES) {
        let _ = writeln!(std::io::stderr().lock(), "[{}] {} ... todo", n, rel_path);
        stats.inc_passed();
        return;
    }

    let io: Rc<VirtualIo> = Rc::new(VirtualIo::with_files(files.clone()));
    // The reference harness runs the compiler with `cwd: <test-dir>` and the
    // input as `input.scss` (lib/compiler.ts:39-44), so error/warning traces
    // are relative to the test directory (basenames). Match that so the
    // normalized paths in the expected files line up.
    io.set_current_dir(test_dir);
    io.set_fallback(Rc::new(DefaultIo::new()));

    let logger: Rc<TestLogger> = Rc::new(TestLogger::new_with_io(io.clone()));

    let mut load_paths: Vec<String> = Vec::new();
    if !spec_root.is_empty() {
        let input_dir = Path::new(abs_input)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        if spec_root != input_dir {
            load_paths.push(spec_root.to_string());
        }
    }

    let io_for_compile: Rc<dyn IoExt> = io.clone();
    let logger_for_compile: Rc<dyn Logger> = logger.clone();
    let arena = Bump::new();

    let compile_opts = CompileOptions {
        load_paths,
        logger: Some(logger_for_compile),
        verbose: true,
        unicode: false,
        charset: true,
        ..CompileOptions::new(&arena)
    };

    // Mode-adaptive drive: async build wraps in block_on; sync build calls
    // directly (the macro reads the unified feature graph, so this matches
    // however rust-sass itself was compiled).
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        rust_sass_macros::maybe_block_on!(compile_stylesheet(
            io_for_compile,
            abs_input,
            "",
            compile_opts,
            &arena,
        ))
    }));

    let result = match result {
        Ok(r) => r,
        Err(e) => {
            let msg = if let Some(s) = e.downcast_ref::<String>() {
                s.clone()
            } else if let Some(s) = e.downcast_ref::<&str>() {
                s.to_string()
            } else {
                "unknown panic".to_string()
            };
            let _ = writeln!(
                std::io::stderr().lock(),
                "[{}] {} ... panic: {}",
                n,
                rel_path,
                msg
            );
            failures.push(format!("{}: panic: {}", rel_path, msg));
            return;
        }
    };

    let output = io.output_buffer();
    let warnings = logger.output();

    let output_content = lookup_impl_file(files, test_dir, "output.css");
    let error_content = lookup_impl_file(files, test_dir, "error");
    let mut had_failure = false;

    if let Some(ref expected_output) = output_content {
        match result {
            Ok(()) => {
                let expected = expected_output.trim().to_string();
                let actual = output.trim().to_string();
                let expected_norm = normalize_output(&expected);
                let actual_norm = normalize_output(&actual);
                if expected_norm != actual_norm {
                    had_failure = true;
                    failures.push(format!(
                        "{}: CSS mismatch\n--- expected:\n{}\n\n--- actual:\n{}",
                        rel_path, expected, output
                    ));
                } else {
                    let warning_content = lookup_impl_file(files, test_dir, "warning");
                    if let Some(expected_warning) = warning_content {
                        stats.inc_warnings_compared();
                        if !opts.is_warning_todo_any(IMPL_NAMES) {
                            let expected_norm = normalize_output(expected_warning.trim());
                            let actual_norm = normalize_output(&warnings);
                            if expected_norm != actual_norm {
                                had_failure = true;
                                failures.push(format!(
                                    "{}: warning mismatch\n--- expected:\n{}\n\n--- actual:\n{}",
                                    rel_path, expected_warning, warnings
                                ));
                            }
                        }
                    }
                }
            }
            Err(ref e) => {
                had_failure = true;
                failures.push(format!("{}: unexpected error: {}", rel_path, e.message));
            }
        }
    }

    if let Some(ref expected_error) = error_content {
        stats.inc_errors_compared();
        match result {
            Ok(()) => {
                had_failure = true;
                failures.push(format!(
                    "{}: expected error but compilation succeeded",
                    rel_path
                ));
            }
            Err(ref e) => {
                // Full-text error comparison, matching the reference harness's
                // strictest mode (dart-sass: `trimErrors: false` in
                // lib/test-case/test-case.ts:126): the entire stderr output is
                // compared. The reference captures the compiler's full stderr,
                // so any warnings emitted before the error (e.g. deprecation
                // warnings for global built-ins) precede the error text.
                // `compile_stylesheet` renders the full error with ASCII glyphs
                // when `unicode: false` (the reference runs `--no-unicode`).
                let expected_norm = normalize_output(expected_error.trim());
                let actual_err = format!("{warnings}{}", e.message);
                let actual_norm = normalize_output(actual_err.trim());
                if expected_norm != actual_norm {
                    had_failure = true;
                    failures.push(format!(
                        "{}: error mismatch\n--- expected:\n{}\n\n--- actual:\n{}",
                        rel_path, expected_error, actual_err
                    ));
                }
            }
        }
    }

    if output_content.is_none() && error_content.is_none() {
        had_failure = true;
        failures.push(format!(
            "{}: test has no expected output (missing output.css or error file)",
            rel_path
        ));
    }

    if had_failure {
        let _ = writeln!(std::io::stderr().lock(), "[{}] {} ... FAIL", n, rel_path);
    } else {
        stats.inc_passed();
        let _ = writeln!(std::io::stderr().lock(), "[{}] {} ... ok", n, rel_path);
    }
}

fn lookup_impl_file(
    files: &HashMap<String, String>,
    test_dir: &str,
    base_name: &str,
) -> Option<String> {
    for &name in IMPL_NAMES {
        let ext_idx = base_name.rfind('.').unwrap_or(base_name.len());
        let base = &base_name[..ext_idx];
        let ext = &base_name[ext_idx..];
        let impl_file = format!("{}/{}-{}{}", test_dir, base, name, ext);
        if files.contains_key(&impl_file) {
            return Some(files[&impl_file].clone());
        }
    }
    let key = format!("{}/{}", test_dir, base_name);
    files.get(&key).cloned()
}

fn normalize_output(output: &str) -> String {
    let output = output.replace("\r\n", "\n");
    let output = RE_CONSECUTIVE_NEWLINES.replace_all(&output, "\n");
    let output = RE_INPUT_PATH.replace_all(&output, "$1");
    output.trim().to_string()
}
