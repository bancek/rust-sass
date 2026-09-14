// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/visitor/evaluate.dart (_EvaluateVisitor constructor — built-in
//   module/function registration; meta-callback owners live in eval/meta.rs)
// go-source: go/eval/eval_init.go

use crate::eval::meta::register_meta_functions;
use bumpalo::Bump;

use crate::common::exception::SassResult;
use crate::eval::EvaluateVisitor;
use crate::functions;
use crate::module::{Module, ModuleKind};

/// Registers built-in modules and global functions on the visitor.
///
/// Rewritten from the tail of Dart's `_EvaluateVisitor` constructor
/// (`visitor/evaluate.dart`): indexes each core module by URL into the
/// built-in module table, then registers every global function twice — once
/// under its hyphen-normalized name in the built-in function table, once in
/// the global overload list. Finally registers the `sass:meta` module
/// (see `eval/meta.rs`). User-supplied `functions` are registered first by
/// the [`evaluate`](crate::eval::evaluate) entry point, so globals overwrite
/// on name collision.
pub fn register_built_in_functions<'compile, 'parse>(
    v: &mut EvaluateVisitor<'compile, 'parse>,
    arena: &'compile Bump,
) -> SassResult<()>
where
    'compile: 'parse,
    'parse: 'compile,
{
    for m in functions::core_modules(arena) {
        let url = m.url.clone();
        v.config
            .built_in_modules
            .insert(url, Module::new(arena, ModuleKind::BuiltIn(m)));
    }

    let global_fns = functions::global_functions(arena);
    for f in global_fns.iter() {
        let normalized = f.name().replace('_', "-");
        v.config
            .built_in_functions
            .borrow_mut()
            .insert(normalized, *f);
    }
    v.config
        .global_functions
        .borrow_mut()
        .extend(global_fns.clone());

    register_meta_functions(v, arena)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::exception::SassResult;
    use crate::compile_context::new_compile_context;
    use crate::io::VirtualIo;
    use crate::logger::Logger;
    use crate::logger::StderrLogger;
    use std::rc::Rc;

    #[test]
    fn test_register_built_in_functions() -> SassResult<()> {
        let io = Rc::new(VirtualIo::new());
        let logger: Rc<dyn Logger> = Rc::new(StderrLogger::new(false, true, io.clone()));
        let arena = Bump::new();
        let mut v = EvaluateVisitor::new(logger, new_compile_context(), &arena, io);

        register_built_in_functions(&mut v, &arena)?;

        // sass:meta should be registered by register_meta_functions
        assert!(v.config.built_in_modules.contains_key("sass:meta"));

        // Global functions should be populated
        assert!(!v.config.global_functions.borrow().is_empty());

        // built_in_functions should have hyphen-normalized keys
        let bf = v.config.built_in_functions.borrow();
        // All keys should use hyphens, not underscores
        for key in bf.keys() {
            assert!(!key.contains('_'), "key '{}' contains underscore", key);
        }

        Ok(())
    }
}
