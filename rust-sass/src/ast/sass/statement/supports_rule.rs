// Copyright 2016 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Ported and rearchitected for Rust by Luka Zakrajsek.

// dart-source: lib/src/ast/sass/statement/supports_rule.dart
// go-source: go/value/sass_statement_supports_rule.go

use std::fmt;
use std::fmt::Write;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

use crate::ast::sass::statement::Statement;
use crate::ast::sass::supports_condition::SupportsCondition;

#[derive(Clone, Debug)]
/// A `@supports` rule.
pub struct SupportsRule<'parse> {
    pub children: Vec<Statement<'parse>>,
    /// The condition selecting which browsers this rule targets.
    pub condition: SupportsCondition<'parse>,
    pub span: FileSpan<'parse>,
}

impl<'parse> SupportsRule<'parse> {
    pub fn new(
        condition: SupportsCondition<'parse>,
        children: Vec<Statement<'parse>>,
        span: FileSpan<'parse>,
    ) -> Self {
        SupportsRule {
            children,
            condition,
            span,
        }
    }
}

impl<'parse> AstNode<'parse> for SupportsRule<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        Ok(self.span)
    }
}

impl<'parse> SupportsRule<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        let mut buf = String::new();
        write!(buf, "@supports {} {{", self.condition).unwrap();
        for child in &self.children {
            write!(buf, " {child}").unwrap();
        }
        write!(buf, " }}").unwrap();
        Ok(buf)
    }
}

impl<'parse> fmt::Display for SupportsRule<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.to_display_string() {
            Ok(s) => f.write_str(&s),
            Err(_) => Err(fmt::Error),
        }
    }
}
