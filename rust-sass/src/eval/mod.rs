// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/visitor/evaluate.dart (evaluate(), Evaluator, _EvaluateVisitor —
//   config/state split across eval/*.rs; see docs/ref/eval.md)
// go-source: go/eval/evaluate.go + go/eval/eval_init.go

use crate::ast::css::comment::CssComment;
use crate::ast::css::import::ModifiableCssImport;
use crate::ast::css::media_query::CssMediaQuery;
use crate::ast::css::style_rule::CssStyleRule;
use crate::common::file_span::BOGUS_SPAN;
use crate::compile_context::new_compile_context;
use crate::eval::importer::ImporterKind;
use crate::eval::init::register_built_in_functions;
use crate::extend::ExtendMode;
use std::cell::RefCell;
use std::collections::HashSet;
use std::fmt;
use std::rc::Rc;

use bumpalo::Bump;
use indexmap::IndexMap;

use crate::ast::sass::expression::Expression;
use crate::ast::sass::statement::{Statement, Stylesheet};
use crate::callable::Callable;
use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;
use crate::compile_context::CompileContext;
use crate::configuration::Configuration;
use crate::environment::Environment;
use crate::io::Io;
use crate::logger::Logger;
use crate::module::Module;
use crate::url::SassUrl;
use crate::value::Value;

// ===========================================================================
// Recursion-edge boxing (maybe-async companion macros)
// docs/ref/macros.md ("Recursion edges")
//
// Async recursion requires boxing (E0733); the boxed edge set is minimal.
// In the sync build (default; `async` feature off) the recursion is
// plain native recursion, so the boxes become direct calls.
//
// Ordering safety: `#[rust_sass_macros::maybe_async]` strips `.await` from
// raw tokens BEFORE these macros expand (attribute macros see
// pre-expansion tokens), so `box_rec!(f()).await` is `Box::pin(f()).await`
// in async mode and `f()` in sync mode.
//
// NOTE: must stay ABOVE the `pub mod` declarations — macro_rules! are
// textually scoped, and the child modules use them.
// ===========================================================================

/// Boxes a recursive eval future in async mode; direct call in sync mode.
#[cfg(feature = "async")]
macro_rules! box_rec {
    ($f:expr) => {
        Box::pin($f)
    };
}
#[cfg(not(feature = "async"))]
macro_rules! box_rec {
    ($f:expr) => {
        $f
    };
}

/// `box_rec!` for arena-pinned futures (`bumpalo::boxed::Box::pin_in`).
#[cfg(feature = "async")]
macro_rules! box_rec_in {
    ($f:expr, $arena:expr $(,)?) => {
        bumpalo::boxed::Box::pin_in($f, $arena)
    };
}
#[cfg(not(feature = "async"))]
macro_rules! box_rec_in {
    ($f:expr, $arena:expr $(,)?) => {
        $f
    };
}

pub mod calc;
pub mod compat;
pub mod css;
pub mod expression;
pub mod helpers;
pub mod import_cache;
pub mod imported_css;
pub mod importer;
pub mod init;
pub mod meta;
pub mod result;
pub mod statement;
pub mod syntax;
pub mod warn;

// ===========================================================================
// StackFrame — linked list for stack traces
// Go: go/eval/evaluate.go:42-47
// ===========================================================================

/// One frame of the dynamic call stack: function/mixin invocations and
/// imports surrounding the current context.
///
/// Rewritten from Dart's `_EvaluateVisitor._stack` entries
/// (`visitor/evaluate.dart`): each entry pairs the member name with the node
/// whose span marks where the trace frame starts. Rust stores the resolved
/// [`FileSpan`] plus the [`Callable`] directly (Dart stores `AstNode`s and
/// manufactures spans lazily); the chain is a `Box`-linked list owned
/// exclusively by the evaluator.
#[derive(Clone)]
pub struct StackFrame<'compile, 'parse> {
    /// Human-readable name of the invoked member.
    pub name: String,
    /// Span where this stack frame starts.
    pub span: FileSpan<'parse>,
    /// The callable being invoked at this frame.
    pub callable: Callable<'compile, 'parse>,
    /// The enclosing frame, if any.
    pub parent: Option<Box<StackFrame<'compile, 'parse>>>,
}

// ===========================================================================
// WarnKey — dedup key for warnings
// Go: go/eval/evaluate.go:53-56 — uses (message, FileSpan) struct key
// Rust: FileSpan has Option<&FileSource> (not Hash). Use (message, source_url, offset).
// ===========================================================================

/// Dedup key for warnings: one warning per message/location.
///
/// Rewritten from Dart's `_EvaluateVisitor._warningsEmitted`
/// (`visitor/evaluate.dart`): Dart keys on `(String, SourceSpan)`.
/// [`FileSpan`] cannot derive `Hash` (its file handle holds references), so
/// Rust keys on the message plus the source URL string, the span's start
/// offset, and the file-source identity (address) to disambiguate
/// same-offset spans from different sources.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct WarnKey {
    pub message: String,
    pub source_url: Option<Rc<str>>,
    pub start_offset: usize,
    /// Identity of the FileSource to distinguish same-offset spans in different sources.
    pub file_id: usize,
}

impl WarnKey {
    pub fn from_span<'parse>(message: &str, span: FileSpan<'parse>) -> Self {
        WarnKey {
            message: message.to_string(),
            // Shared, formatted once per source (see `FileSource::url_str_cached`).
            source_url: span.url_str_cached(),
            start_offset: span.start_location().offset,
            file_id: span.file().map(|fs| fs as *const _ as usize).unwrap_or(0),
        }
    }
}

// ===========================================================================
// EvalConfig — immutable shared resources
//
// Split-rule owner for the config half of Dart's `_EvaluateVisitor`
// (visitor/evaluate.dart): the fields below that never change during a
// compilation (registries, logger, flags, host seams) correspond to Dart's
// `_builtInFunctions`/`_builtInModules`, `_logger`, `_quietDeps`,
// `_sourceMap`, `_compileContext`, `_nodeImporter` members. All evaluation
// logic takes `config` by shared reference in free functions
// (`fn(config, state, arena, …)`); see docs/ref/eval.md.
// ===========================================================================

/// Immutable resources shared across a whole compilation.
///
/// Rewritten from the config half of Dart's `_EvaluateVisitor`
/// (`visitor/evaluate.dart`): built-in registries, the logger, the
/// compile-context identity token, and the CLI flags. Passed by shared
/// reference into every evaluation free function alongside the mutable
/// [`EvalState`].
#[derive(Clone)]
pub struct EvalConfig<'compile, 'parse> {
    /// Built-in modules indexed by URL (`sass:math`, `sass:meta`, …).
    ///
    /// Matches Dart: `_EvaluateVisitor._builtInModules`.
    pub built_in_modules: IndexMap<String, Module<'compile, 'parse>>,
    /// Shared mutable via arena `&'parse RefCell` — meta callbacks capture the
    /// reference (Copy).
    /// Built-in functions available globally, even under the module system,
    /// indexed by hyphen-normalized name. Shared mutable via arena `&'parse
    /// RefCell` — meta callbacks capture the reference (Copy).
    ///
    /// Matches Dart: `_EvaluateVisitor._builtInFunctions`.
    pub built_in_functions: &'parse RefCell<IndexMap<String, Callable<'compile, 'parse>>>,
    /// User-supplied callables available as global functions. Shared mutable
    /// via arena `&'parse RefCell` — meta callbacks capture the reference
    /// (Copy).
    pub global_functions: &'parse RefCell<Vec<Callable<'compile, 'parse>>>,
    /// Logger used to print warnings.
    ///
    /// Matches Dart: `_EvaluateVisitor._logger`.
    pub logger: Rc<dyn Logger>,
    /// Whether to avoid emitting warnings for files loaded from dependencies.
    ///
    /// Matches Dart: `_EvaluateVisitor._quietDeps`.
    pub quiet_deps: bool,
    /// Whether to track source map information (variable-declaration sources).
    ///
    /// Matches Dart: `_EvaluateVisitor._sourceMap`.
    pub source_map: bool,
    /// Unique token identifying this compilation; callables check it to decide
    /// whether they belong to the current compilation.
    ///
    /// Matches Dart: `_EvaluateVisitor._compileContext`.
    pub compile_context: CompileContext,
    // Dead field (U13 suspect, resolved): no Dart counterpart, never read
    // outside `Debug`. Kept so `EvalConfig` construction sites stay stable;
    // the real bound is the machine stack (see critical-invariants.md
    // "Stack safety"). Retained deliberately — do not wire it up.
    pub max_recursion_depth: u32,
    /// The Node Sass-compatible importer used when loading new Sass files.
    ///
    /// Matches Dart: `_EvaluateVisitor._nodeImporter`. Stored as a presence
    /// flag: when set, `stdin` URLs are not recorded in `loaded_urls` and the
    /// legacy `NodeImporter` path applies (Dart: `_EvaluateVisitor._asNodeSass`).
    pub node_importer: Option<&'parse NodePackageImporter>,
    /// Host filesystem seam, threaded through config rather than per function.
    pub io: Rc<dyn Io>,
    /// `--unicode` from the CLI; drives the glyph set for embedded error text
    /// (e.g. selector parse errors rendered inside a Script error's message).
    pub unicode: bool,
    pub alert_color: bool,
    pub alert_ascii: bool,
}

impl<'compile: 'parse, 'parse> fmt::Debug for EvalConfig<'compile, 'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EvalConfig")
            .field("quiet_deps", &self.quiet_deps)
            .field("source_map", &self.source_map)
            .field("max_recursion_depth", &self.max_recursion_depth)
            .finish()
    }
}

impl<'compile: 'parse, 'parse> EvalConfig<'compile, 'parse> {
    pub fn new(
        arena: &'compile Bump,
        logger: Rc<dyn Logger>,
        compile_context: CompileContext,
        io: Rc<dyn Io>,
    ) -> Self {
        EvalConfig {
            built_in_modules: IndexMap::new(),
            built_in_functions: &*arena.alloc(RefCell::new(IndexMap::new())),
            global_functions: &*arena.alloc(RefCell::new(Vec::new())),
            logger,
            quiet_deps: false,
            source_map: false,
            compile_context,
            max_recursion_depth: 250,
            node_importer: None,
            io,
            unicode: true,
            alert_color: false,
            alert_ascii: false,
        }
    }
}

// ===========================================================================
// EvalState — all mutable runtime state
//
// Split-rule owner for the state half of Dart's `_EvaluateVisitor`
// (visitor/evaluate.dart): the current environment, stack, CSS output
// position, module tables, warning spans, and load tracking below correspond
// to Dart's mutable `_environment`, `_stack`, `_parent`, `_modules`, …
// members. Threaded by mutable reference into the same free functions that
// take `&EvalConfig`; see docs/ref/eval.md.
// ===========================================================================

/// Mutable runtime state for one evaluation.
///
/// Rewritten from the state half of Dart's `_EvaluateVisitor`
/// (`visitor/evaluate.dart`). All evaluation free functions take `state` by
/// mutable reference alongside `&EvalConfig`.
pub struct EvalState<'compile, 'parse> {
    // ── Environment & stack ──
    /// The current lexical environment.
    ///
    /// Matches Dart: `_EvaluateVisitor._environment`.
    pub env: Environment<'compile, 'parse>,
    /// The dynamic call stack (function/mixin invocations and imports).
    ///
    /// Matches Dart: `_EvaluateVisitor._stack`.
    pub stack: Option<Box<StackFrame<'compile, 'parse>>>,
    /// Human-readable name of the current stack frame (`"root stylesheet"` at
    /// the entrypoint).
    ///
    /// Matches Dart: `_EvaluateVisitor._member`.
    pub member: String,

    // ── Warning/deprecation span fallbacks ──
    /// Span of the current @import being evaluated; used for importer warnings.
    ///
    /// Matches Dart: `_EvaluateVisitor._importSpan`.
    pub import_span: Option<FileSpan<'parse>>,
    /// Node for the innermost callable being invoked; used for function-call
    /// warnings. Stored as a span directly (Dart keeps the `AstNode` to avoid
    /// manufacturing spans eagerly; set by `invoke_callable`).
    ///
    /// Matches Dart: `_EvaluateVisitor._callableNode`.
    pub callable_span: Option<FileSpan<'parse>>,
    /// Fallback span when neither import nor callable is available.
    ///
    /// Matches Dart: `_EvaluationContext`'s default warn node with span.
    pub default_warn_span: FileSpan<'parse>,

    pub recursion_depth: u32,

    // ── Module system ──
    /// All modules loaded and evaluated so far.
    ///
    /// Matches Dart: `_EvaluateVisitor._modules`.
    pub modules: IndexMap<String, Module<'compile, 'parse>>,
    /// The first configuration used to load each module URL.
    ///
    /// Matches Dart: `_EvaluateVisitor._moduleConfigurations`.
    pub module_configurations: IndexMap<String, Configuration<'parse>>,
    /// Canonical URLs of modules currently being evaluated, for loop detection.
    ///
    /// Matches Dart: `_EvaluateVisitor._activeModules`.
    pub active_modules: IndexMap<String, Option<FileSpan<'parse>>>,
    /// Configuration for the current module (`!default` overrides).
    ///
    /// Matches Dart: `_EvaluateVisitor._configuration`.
    pub configuration: Configuration<'parse>,
    /// Nodes whose spans indicate where each module was originally loaded.
    ///
    /// Matches Dart: `_EvaluateVisitor._moduleNodes`.
    pub module_nodes: IndexMap<String, Option<FileSpan<'parse>>>,
    /// Comments collected between @use/@forward rules for the current module.
    pub pre_module_comments: Option<IndexMap<Module<'compile, 'parse>, Vec<CssComment<'parse>>>>,

    // ── CSS output tree ──
    /// The root stylesheet node of the output CSS tree.
    ///
    /// Matches Dart: `_EvaluateVisitor._root`.
    pub root: Option<ModifiableCssNode<'parse>>,
    /// The current parent node in the output CSS tree.
    ///
    /// Matches Dart: `_EvaluateVisitor._parent`.
    pub parent: Option<ModifiableCssNode<'parse>>,
    /// First index in the root children after the initial block of CSS imports.
    ///
    /// Matches Dart: `_EvaluateVisitor._endOfImports`.
    pub end_of_imports: usize,
    /// Plain-CSS imports that appeared after the initial import block; hoisted
    /// back into it after the stylesheet is fully evaluated.
    ///
    /// Matches Dart: `_EvaluateVisitor._outOfOrderImports`.
    pub out_of_order_imports: Vec<ModifiableCssImport<'parse>>,
    #[allow(dead_code)]
    pub(crate) extension_store: Option<DefaultExtensionStore<'parse>>,

    // ── @at-root / media query context ──
    /// Style rule defining the current parent selector, ignoring intermediate
    /// `@at-root` rules.
    ///
    /// Matches Dart: `_EvaluateVisitor._styleRuleIgnoringAtRoot`.
    pub style_rule_ignoring_at_root: Option<CssStyleRule<'parse>>,
    /// Whether we're directly within an `@at-root` that excludes style rules.
    ///
    /// Matches Dart: `_EvaluateVisitor._atRootExcludingStyleRule`.
    pub at_root_excluding_style_rule: bool,
    /// The current media queries, if any.
    ///
    /// Matches Dart: `_EvaluateVisitor._mediaQueries`.
    pub media_queries: Option<Vec<CssMediaQuery>>,
    /// Media queries merged to create [`EvalState::media_queries`].
    ///
    /// Matches Dart: `_EvaluateVisitor._mediaQuerySources`.
    pub media_query_sources: Option<Vec<CssMediaQuery>>,

    /// The modifiable CSS output node for the current style rule, set by the
    /// CSS re-evaluation visitor. Used by visitCssMediaRule/visitCssSupportsRule
    /// for copyWithoutChildren. None outside the CSS re-evaluation context.
    pub css_style_rule_node: Option<ModifiableCssNode<'parse>>,

    // ── libsass NESTED tabs collection ──
    /// Hoisted style/media/supports nodes awaiting NESTED `tabs` stamping,
    /// paired with the opaque-nesting generation at attach time. No Dart
    /// counterpart (libsass `Cssize` tabs accumulation in
    /// `libsass/src/cssize.cpp`).
    ///
    /// Entries are registered by [`add_child`](super::helpers::add_child) as
    /// nodes attach; style-rule frames stamp their range on completion and
    /// media/supports frames drop their inner range as opaque. Entries for
    /// dead subtrees linger harmlessly (the compile aborts or the state
    /// drops); stamps live on the shared nodes, not here.
    pub tabs_pending: Vec<(ModifiableCssNode<'parse>, u32)>,
    /// Opaque-nesting depth for [`tabs_pending`](Self::tabs_pending):
    /// media/supports scopes bump it so enclosing style frames cannot reach
    /// inside (libsass bubble boundaries); style rules and `@at-root` are
    /// transparent and never bump it.
    pub tabs_gen: u32,
    /// Open style-rule frames for NESTED stamping, innermost-last: the
    /// frame's own node plus its [`tabs_pending`](Self::tabs_pending) start
    /// index. Stamping skips the frame's own node and its subtree (content
    /// emitted in place needs no extra indent).
    pub tabs_frames: Vec<(ModifiableCssNode<'parse>, usize)>,

    // ── Declaration / flag context ──
    /// Name of the current declaration parent, if any.
    ///
    /// Matches Dart: `_EvaluateVisitor._declarationName`.
    pub declaration_name: Option<String>,
    /// Whether we're currently executing a function.
    ///
    /// Matches Dart: `_EvaluateVisitor._inFunction`.
    pub in_function: bool,
    /// Whether we're currently executing a mixin.
    pub in_mixin: bool,
    /// Whether we're currently building the output of an unknown at-rule.
    ///
    /// Matches Dart: `_EvaluateVisitor._inUnknownAtRule`.
    pub in_unknown_at_rule: bool,
    /// Whether we're currently building the output of a `@keyframes` rule.
    ///
    /// Matches Dart: `_EvaluateVisitor._inKeyframes`.
    pub in_keyframes: bool,
    /// Whether we're evaluating a supports declaration (calculations are not
    /// simplified there).
    ///
    /// Matches Dart: `_EvaluateVisitor._inSupportsDeclaration`.
    pub in_supports_declaration: bool,
    /// Whether we're in a dependency (a stylesheet imported by an importer
    /// other than the original).
    ///
    /// Matches Dart: `_EvaluateVisitor._inDependency`.
    pub in_dependency: bool,

    // ── Warning dedup ──
    /// Message/location pairs for warnings already emitted; one warning is
    /// emitted per location.
    ///
    /// Matches Dart: `_EvaluateVisitor._warningsEmitted`.
    pub warnings_emitted: HashSet<WarnKey>,

    // ── Import / URL tracking ──
    /// Cache used to import other stylesheets. Flows by ownership: created in
    /// `compile_string`, installed here by [`evaluate`], and returned on the
    /// result.
    ///
    /// Matches Dart: `_EvaluateVisitor._importCache`.
    pub import_cache: Option<ImportCache<'compile, 'parse>>,
    /// Importer currently resolving relative imports (`None` support means
    /// relative imports are unsupported in the current stylesheet).
    ///
    /// Matches Dart: `_EvaluateVisitor._importer`.
    pub importer: Importer<'parse>,
    /// Canonical URL of the stylesheet being evaluated, if it has one.
    pub source_url: Option<SassUrl>,
    /// Canonical URLs of all stylesheets loaded during compilation.
    ///
    /// Matches Dart: `_EvaluateVisitor._loadedUrls`.
    pub loaded_urls: Vec<String>,

    // ── Current stylesheet ──
    /// Stylesheet currently being evaluated.
    ///
    /// Matches Dart: `_EvaluateVisitor._stylesheet`.
    pub stylesheet: Option<&'parse Stylesheet<'parse>>,
}

impl<'compile: 'parse, 'parse> fmt::Debug for EvalState<'compile, 'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EvalState")
            .field("member", &self.member)
            .field("recursion_depth", &self.recursion_depth)
            .finish()
    }
}

impl<'compile: 'parse, 'parse> EvalState<'compile, 'parse> {
    pub fn new(_arena: &'compile Bump, default_warn_span: FileSpan<'parse>) -> Self {
        EvalState {
            env: Environment::new(_arena),
            stack: None,
            member: "root stylesheet".to_string(),
            import_span: None,
            callable_span: None,
            default_warn_span,
            recursion_depth: 0,
            modules: IndexMap::new(),
            module_configurations: IndexMap::new(),
            active_modules: IndexMap::new(),
            configuration: Configuration::empty(_arena),
            module_nodes: IndexMap::new(),
            pre_module_comments: None,
            root: None,
            parent: None,
            end_of_imports: 0,
            out_of_order_imports: Vec::new(),
            extension_store: None,
            style_rule_ignoring_at_root: None,
            at_root_excluding_style_rule: false,
            media_queries: None,
            media_query_sources: None,
            css_style_rule_node: None,
            tabs_pending: Vec::new(),
            tabs_gen: 0,
            tabs_frames: Vec::new(),
            declaration_name: None,
            in_function: false,
            in_mixin: false,
            in_unknown_at_rule: false,
            in_keyframes: false,
            in_supports_declaration: false,
            in_dependency: false,
            warnings_emitted: HashSet::new(),
            import_cache: None,
            importer: Importer::new(_arena, ImporterKind::NoOp),
            source_url: None,
            loaded_urls: Vec::new(),
            stylesheet: None,
        }
    }
}

// ===========================================================================
// ImportCache — re-exported at crate::eval::ImportCache
// ===========================================================================

// EvalConfig stores `Option<&'parse ImportCache<'parse>>`.
// ImportCache is defined in eval::import_cache, same parent module.

// ===========================================================================
// EvaluateVisitor — config + state with visitor trait impls
//
// Split-rule owner for Dart's `_EvaluateVisitor` identity: in Rust the type
// is only the `{config, state, arena}` bundle passed (split) into the free
// evaluation functions. There are no visitor-trait impls on it; call sites
// borrow `&v.config` plus `&mut v.state` directly. Matches Dart:
// visitor/evaluate.dart `_EvaluateVisitor` (constructor ~line 370).
// ===========================================================================

/// Bundle of [`EvalConfig`], [`EvalState`], and the compile arena.
///
/// Rewritten from Dart's `_EvaluateVisitor` (`visitor/evaluate.dart`): only
/// constructors and the [`evaluate`] entry points are methods here; all
/// statement/expression/CSS logic lives in free functions taking
/// `(config, state, arena, …)`.
pub struct EvaluateVisitor<'compile, 'parse> {
    pub config: EvalConfig<'compile, 'parse>,
    pub state: EvalState<'compile, 'parse>,
    pub arena: &'compile Bump,
}

impl<'compile: 'parse, 'parse> EvaluateVisitor<'compile, 'parse> {
    pub fn new(
        logger: Rc<dyn Logger>,
        compile_context: CompileContext,
        arena: &'compile Bump,
        io: Rc<dyn Io>,
    ) -> Self {
        EvaluateVisitor {
            config: EvalConfig::new(arena, logger, compile_context, io),
            state: EvalState::new(arena, BOGUS_SPAN),
            arena,
        }
    }
}

impl<'compile: 'parse, 'parse> fmt::Debug for EvaluateVisitor<'compile, 'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EvaluateVisitor")
            .field("config", &self.config)
            .field("state", &self.state)
            .finish()
    }
}

// ===========================================================================
// evaluate() — top-level free function
// Matches Dart: evaluate.dart:evaluate() lines 90-107
// Matches Go: eval.Evaluate() + EvaluateVisitor.Run()
// ===========================================================================

use crate::ast::css::modifiable_node::{ModifiableCssNode, ModifiableCssNodeKind};
use crate::ast::css::stylesheet::ModifiableCssStylesheet;
use crate::eval::import_cache::ImportCache;
use crate::eval::importer::node_package::NodePackageImporter;
use crate::eval::importer::Importer;
use crate::eval::result::EvaluateResult;
use crate::extend::store::{DefaultExtensionStore, ExtensionStore};

/// Converts a stylesheet to a plain CSS tree.
///
/// If `import_cache` is passed, it is used to resolve imports in the Sass
/// files. If `importer` is passed, it resolves relative imports relative to
/// the stylesheet's source URL. `functions` are available as globals, warnings
/// go through `logger`, and `source_map` tracks variable-declaration sources.
///
/// Rewritten from Dart's top-level `evaluate()` (`visitor/evaluate.dart`).
// Arity mirrors Dart's `evaluate()` options threading; a params struct would
// diverge from the pinned Dart/Go signatures.
#[allow(clippy::too_many_arguments)]
#[rust_sass_macros::maybe_async]
pub async fn evaluate<'compile, 'parse>(
    stylesheet: &'parse Stylesheet<'parse>,
    import_cache: Option<ImportCache<'compile, 'parse>>,
    node_importer: Option<&'parse NodePackageImporter>,
    importer: Importer<'parse>,
    functions: Vec<Callable<'compile, 'parse>>,
    logger: Rc<dyn Logger>,
    quiet_deps: bool,
    source_map: bool,
    unicode: bool,
    alert_color: bool,
    alert_ascii: bool,
    arena: &'compile Bump,
    io: Rc<dyn Io>,
) -> SassResult<EvaluateResult<'compile, 'parse>>
where
    'compile: 'parse,
    'parse: 'compile,
{
    let mut v = EvaluateVisitor::new(logger.clone(), new_compile_context(), arena, io);
    v.config.unicode = unicode;
    v.config.alert_color = alert_color;
    v.config.alert_ascii = alert_ascii;

    // Set import_cache (matching Go's NewEvaluateVisitor parameter)
    v.state.import_cache = import_cache;

    // Register user functions (matching Go's RegisterUserFunctions)
    for f in functions.iter() {
        let normalized = f.name().replace('_', "-");
        v.config
            .built_in_functions
            .borrow_mut()
            .insert(normalized, *f);
    }
    {
        let mut gf = v.config.global_functions.borrow_mut();
        for f in functions.iter() {
            gf.push(*f);
        }
    }

    // Register built-in functions (matching Go's RegisterBuiltInFunctions)
    init::register_built_in_functions(&mut v, arena)?;

    // Set flags
    v.config.quiet_deps = quiet_deps;
    v.config.source_map = source_map;
    v.config.node_importer = node_importer;

    // Set up CSS output, eval state, and evaluate stylesheet. Wrapped in
    // with_evaluation_context so that the stylesheet's span is the default
    // fallback for deprecation warnings (Dart: run() line 714).
    let node_span = stylesheet.span;
    helpers::with_evaluation_context(&v.config, &mut v.state, node_span, async |config, state| {
        let ss = ModifiableCssStylesheet::new(node_span);
        let root = ModifiableCssNode::new(arena, ModifiableCssNodeKind::Stylesheet(ss));
        state.root = Some(root.clone());
        state.parent = Some(root);

        // Init loaded_urls + source_url
        state.loaded_urls = Vec::new();
        if let Some(url) = stylesheet.span.source_url() {
            let is_node_stdin = config.node_importer.is_some() && url.as_str() == "stdin";
            if !is_node_stdin {
                let url_str = url.to_string();
                if !state.loaded_urls.contains(&url_str) {
                    state.loaded_urls.push(url_str);
                }
            }
            state.source_url = Some(url.clone());
        }

        // Set importer + stylesheet + extension_store
        state.importer = importer;
        state.stylesheet = Some(stylesheet);
        state.extension_store = Some(DefaultExtensionStore::new(
            ExtendMode::Normal,
            config.io.clone(),
            config.unicode,
        ));

        // Active modules entry
        if let Some(url) = stylesheet.span.source_url() {
            state.active_modules.insert(url.to_string(), None);
        }

        // Evaluate (Dart: _addExceptionTrace -> _execute; attach loaded URLs on error)
        let result = helpers::add_exception_trace(state, async |state| {
            statement::evaluate_stylesheet(config, state, arena, stylesheet).await
        })
        .await;
        match result {
            Ok(v) => Ok(v.map(|_| ())),
            // Dart evaluate.dart:728-730 stamps `withLoadedUrls` on every
            // `SassException` — all spanned variants, not just `Runtime`.
            Err(e) => Err(Box::new(
                e.with_loaded_urls(helpers::loaded_urls_list(state)),
            )),
        }
    })
    .await?;

    // Build CSS output
    let root_ref = v.state.root.as_ref().unwrap();
    let css_children = if !v.state.out_of_order_imports.is_empty() {
        statement::build_out_of_order_children(arena, &v.state)
    } else {
        root_ref.children().unwrap_or_default()
    };
    let css = ModifiableCssNode::new(
        arena,
        ModifiableCssNodeKind::Stylesheet(ModifiableCssStylesheet::new(node_span)),
    );
    for child in &css_children {
        css.add_child(child)?;
    }

    // Build root module + combine CSS
    let ext_store = v
        .state
        .extension_store
        .take()
        .map(|es| ExtensionStore::Default(Rc::new(RefCell::new(es))))
        .unwrap_or(ExtensionStore::Empty);
    let pre_mod = v.state.pre_module_comments.take().unwrap_or_default();
    let root_module = v.state.env.to_module(arena, &css, &pre_mod, ext_store);
    let combined = statement::combine_css(arena, &root_module, false)?;

    // Build result
    let loaded_urls: Vec<SassUrl> = v
        .state
        .loaded_urls
        .iter()
        .filter_map(|s| SassUrl::parse(s).ok())
        .collect();

    Ok(EvaluateResult {
        stylesheet: combined,
        loaded_urls,
        import_cache: v.state.import_cache,
    })
}

// ===========================================================================
// runExpression — evaluates an expression in an isolated temporary context
//
// Rewritten from Dart's `_EvaluateVisitor.runExpression`
// (visitor/evaluate.dart). Sets a fake stylesheet/importer around the
// expression so relative loads resolve, then adds the exception trace.
// ===========================================================================

#[rust_sass_macros::maybe_async]
pub(crate) async fn run_expression<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    importer: Importer<'parse>,
    expression: &Expression<'parse>,
) -> SassResult<Value<'parse>>
where
    'compile: 'parse,
{
    let span = expression.span()?;
    helpers::with_evaluation_context(config, state, span, async |config, state| {
        helpers::with_fake_stylesheet(
            config,
            state,
            arena,
            importer,
            span,
            async |config, state| {
                helpers::add_exception_trace(state, async |state| {
                    expression::evaluate_expression(config, state, arena, expression).await
                })
                .await
            },
        )
        .await
    })
    .await
}

// ===========================================================================
// runStatement — evaluates a statement in an isolated temporary context
//
// Rewritten from Dart's `_EvaluateVisitor.runStatement`
// (visitor/evaluate.dart). Same fake-stylesheet/trace wrapping as
// [`run_expression`], for statements.
// ===========================================================================

#[rust_sass_macros::maybe_async]
pub(crate) async fn run_statement<'compile, 'parse>(
    config: &EvalConfig<'compile, 'parse>,
    state: &mut EvalState<'compile, 'parse>,
    arena: &'compile Bump,
    importer: Importer<'parse>,
    statement: &Statement<'parse>,
) -> SassResult<Option<Value<'parse>>>
where
    'compile: 'parse,
{
    let span = statement.span()?;
    helpers::with_evaluation_context(config, state, span, async |config, state| {
        helpers::with_fake_stylesheet(
            config,
            state,
            arena,
            importer,
            span,
            async |config, state| {
                helpers::add_exception_trace(state, async |state| {
                    statement::evaluate_statement(config, state, arena, statement).await
                })
                .await
            },
        )
        .await
    })
    .await
}

// ===========================================================================
// Evaluator — public API for incremental expression/statement evaluation
//
// Rewritten from Dart's `Evaluator` class (`visitor/evaluate.dart`): evaluates
// multiple independent statements/expressions in one module context. There is
// no `use` method (Dart REPL-only, no Rust caller — accepted gap).
// ===========================================================================

/// Evaluates independent expressions and statements in one module context.
///
/// Rewritten from Dart's `Evaluator` (`visitor/evaluate.dart`); arguments
/// match [`evaluate`].
pub struct Evaluator<'compile, 'parse> {
    visitor: EvaluateVisitor<'compile, 'parse>,
    importer: Importer<'parse>,
}

impl<'compile: 'parse, 'parse> Evaluator<'compile, 'parse> {
    // Arguments match [`evaluate`]; a params struct would diverge from that shape.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        import_cache: Option<ImportCache<'compile, 'parse>>,
        importer: Importer<'parse>,
        functions: Vec<Callable<'compile, 'parse>>,
        node_importer: Option<&'parse NodePackageImporter>,
        logger: Rc<dyn Logger>,
        quiet_deps: bool,
        source_map: bool,
        arena: &'compile Bump,
        io: Rc<dyn Io>,
    ) -> SassResult<Self>
    where
        'parse: 'compile,
    {
        let compile_context = new_compile_context();
        let mut visitor = EvaluateVisitor::new(logger, compile_context, arena, io.clone());

        visitor.state.import_cache = import_cache;
        visitor.config.quiet_deps = quiet_deps;
        visitor.config.source_map = source_map;
        visitor.config.node_importer = node_importer;

        for f in functions {
            let normalized = f.name().replace('_', "-");
            visitor
                .config
                .built_in_functions
                .borrow_mut()
                .insert(normalized, f);
            visitor.config.global_functions.borrow_mut().push(f);
        }

        register_built_in_functions(&mut visitor, arena)?;

        Ok(Evaluator { visitor, importer })
    }

    /// Evaluates a Sass expression in the evaluator's context.
    #[rust_sass_macros::maybe_async]
    pub async fn evaluate(&mut self, expression: &Expression<'parse>) -> SassResult<Value<'parse>> {
        run_expression(
            &self.visitor.config,
            &mut self.visitor.state,
            self.visitor.arena,
            self.importer,
            expression,
        )
        .await
    }

    /// Processes a variable declaration in the evaluator's context.
    #[rust_sass_macros::maybe_async]
    pub async fn set_variable(&mut self, declaration: &Statement<'parse>) -> SassResult<()> {
        run_statement(
            &self.visitor.config,
            &mut self.visitor.state,
            self.visitor.arena,
            self.importer,
            declaration,
        )
        .await
        .map(|_| ())
    }
}

#[cfg(test)]
mod evaluate_tests {
    use super::*;
    use crate::ast::sass::expression_number::NumberExpression;
    use crate::common::file_span::SourceLocation;
    use crate::common::file_span::BOGUS_SPAN;
    use crate::common::source_span_file_source::FileSource;
    use crate::common::source_span_span_with_context::SourceSpanWithContext;
    use crate::common::SassError;
    use crate::compile::compile;
    use crate::compile::CompileOptions;
    use crate::eval::importer::ImporterKind;
    use crate::io::VirtualIo;
    use crate::logger::QuietLogger;
    use crate::parse::parser::ParseError;
    use crate::parse::stylesheet::{StylesheetParser, Syntax};
    use crate::value::ValueKind;
    use std::collections::HashMap;

    #[rust_sass_macros::maybe_async]
    async fn test_eval<'compile, 'parse>(
        source: &str,
        arena: &'compile Bump,
    ) -> SassResult<EvaluateResult<'compile, 'parse>>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, source, None);
        let stylesheet = arena.alloc(
            match StylesheetParser::new(fs, Syntax::Scss, None).parse() {
                Ok(s) => s,
                Err(e) => match *e {
                    ParseError::Sass(inner) => return Err(inner),
                    other => {
                        return Err(Box::new(SassError::Script {
                            message: other.to_string(),
                            argument_name: None,
                        }));
                    }
                },
            },
        );
        let logger: Rc<dyn Logger> = Rc::new(QuietLogger);
        evaluate(
            stylesheet,
            None,
            None,
            Importer::new(arena, ImporterKind::NoOp),
            vec![],
            logger,
            false,
            false,
            true,
            false,
            false,
            arena,
            Rc::new(VirtualIo::new()),
        )
        .await
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_empty() {
        let arena = Bump::new();
        let result = test_eval("", &arena).await.unwrap();
        assert!(result.stylesheet.children.is_empty());
        assert!(result.loaded_urls.is_empty());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_basic_rule() {
        let arena = Bump::new();
        let result = test_eval("a { color: red; }", &arena).await.unwrap();
        assert!(
            !result.stylesheet.children.is_empty(),
            "expected CSS output"
        );
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_variable() {
        let arena = Bump::new();
        let result = test_eval("$x: 10px; a { width: $x; }", &arena)
            .await
            .unwrap();
        assert!(!result.stylesheet.children.is_empty());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_error_invalid() {
        let arena = Bump::new();
        let result = test_eval("$x: 1px + red;", &arena).await;
        assert!(result.is_err(), "expected error for invalid SCSS");
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_loaded_urls() {
        let arena = Bump::new();
        let result = test_eval("a { color: red; }", &arena).await.unwrap();
        // loaded_urls may be empty for stdin (e.g. test_eval without source URL)
        assert!(result.loaded_urls.is_empty());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluate_result_fields() {
        let arena = Bump::new();
        let result = test_eval("a { color: red; }", &arena).await.unwrap();
        // Stylesheet should have children
        assert!(!result.stylesheet.children.is_empty());
    }

    #[rust_sass_macros::maybe_test]
    async fn test_evaluator_evaluate_expression() {
        let arena = Bump::new();
        let logger: Rc<dyn Logger> = Rc::new(QuietLogger);
        let io = Rc::new(VirtualIo::new());
        let mut evaluator = Evaluator::new(
            None,
            Importer::new(&arena, ImporterKind::NoOp),
            vec![],
            None,
            logger,
            false,
            false,
            &arena,
            io,
        )
        .unwrap();

        let expr = Expression::Number(NumberExpression {
            value: 1.0,
            unit: Some("px".to_string()),
            span: BOGUS_SPAN,
        });
        let result = evaluator.evaluate(&expr).await.unwrap();
        assert!(matches!(*result, ValueKind::Number(_)));
    }

    // Dart evaluate.dart:728-730 stamps `withLoadedUrls` on every
    // `SassException`: an entrypoint with a `url` that fails with
    // `"Can't find stylesheet to import."` (a `Sass` at the load site, stamped
    // at this boundary) must carry the entrypoint URL. `with_additional_span`
    // must preserve those URLs through the selector error path.
    //
    // Sensitivity: reverting either half fails this test — the boundary stamp
    // revert was masked here only because `add_exception_span` re-stamps the
    // inner load to `Runtime` with `vec![]`; the `Sass`-variant coverage is
    // pinned by the embedded failure-response path (`compilation.rs:490`).
    #[rust_sass_macros::maybe_test]
    async fn test_loaded_urls_on_sass_error() {
        let arena = Bump::new();
        let mut files = HashMap::new();
        files.insert(
            "/entry.scss".to_string(),
            "@use \"nonexistent\";".to_string(),
        );
        let io: Rc<dyn Io> = Rc::new(VirtualIo::with_files(files));
        let mut opts = CompileOptions::new(&arena);
        opts.url = Some(SassUrl::parse("file:///entry.scss").unwrap());
        let err = compile("/entry.scss", io, opts, &arena).await.unwrap_err();
        // `Sass` at the throw site, `Runtime` with trace at the boundary —
        // either way the entrypoint URL must be stamped (pre-fix: `vec![]`).
        assert!(
            err.message().contains("Can't find stylesheet to import."),
            "got: {}",
            err.message()
        );
        assert!(
            err.loaded_urls()
                .iter()
                .any(|u| u.to_string().contains("entry.scss")),
            "got: {:?}",
            err.loaded_urls()
        );

        // `with_additional_span` preserves loaded URLs.
        let span = SourceSpanWithContext::new(
            SourceLocation {
                offset: 0,
                line: 0,
                column: 0,
            },
            SourceLocation {
                offset: 1,
                line: 0,
                column: 1,
            },
            "x".into(),
            "x".into(),
            None,
        )
        .unwrap();
        let stamped = SassError::Sass {
            message: "m".into(),
            span: span.clone(),
            cause: None,
            loaded_urls: vec![SassUrl::parse("file:///entry.scss").unwrap()],
        }
        .with_additional_span(span, "extra".into());
        assert!(
            stamped
                .loaded_urls()
                .iter()
                .any(|u| u.to_string().contains("entry.scss")),
            "got: {:?}",
            stamped.loaded_urls()
        );
    }
}
