pub mod at_root_rule;
pub mod at_rule;
pub mod callable_declaration;
pub mod content_block;
pub mod content_rule;
pub mod debug_rule;
pub mod declaration;
pub mod each_rule;
pub mod error_rule;
pub mod extend_rule;
pub mod for_rule;
pub mod forward_rule;
pub mod function_rule;
pub mod if_rule;
pub mod import_rule;
pub mod include_rule;
pub mod loud_comment;
pub mod media_rule;
pub mod mixin_rule;
pub mod return_rule;
pub mod silent_comment;
pub mod style_rule;
pub mod stylesheet;
pub mod supports_rule;
pub mod use_rule;
pub mod variable_declaration;
pub mod warn_rule;
pub mod while_rule;

use std::fmt;

use crate::common::ast_node::AstNode;
use crate::common::exception::SassResult;
use crate::common::file_span::FileSpan;

pub use crate::ast::sass::statement::at_root_rule::AtRootRule;
pub use crate::ast::sass::statement::at_rule::AtRule;
pub use crate::ast::sass::statement::callable_declaration::CallableDeclaration;
pub use crate::ast::sass::statement::content_block::ContentBlock;
pub use crate::ast::sass::statement::content_rule::ContentRule;
pub use crate::ast::sass::statement::debug_rule::DebugRule;
pub use crate::ast::sass::statement::declaration::Declaration;
pub use crate::ast::sass::statement::each_rule::EachRule;
pub use crate::ast::sass::statement::error_rule::ErrorRule;
pub use crate::ast::sass::statement::extend_rule::ExtendRule;
pub use crate::ast::sass::statement::for_rule::ForRule;
pub use crate::ast::sass::statement::forward_rule::ForwardRule;
pub use crate::ast::sass::statement::function_rule::FunctionRule;
pub use crate::ast::sass::statement::if_rule::IfClause;
pub use crate::ast::sass::statement::if_rule::IfRule;
pub use crate::ast::sass::statement::import_rule::ImportRule;
pub use crate::ast::sass::statement::include_rule::IncludeRule;
pub use crate::ast::sass::statement::loud_comment::LoudComment;
pub use crate::ast::sass::statement::media_rule::MediaRule;
pub use crate::ast::sass::statement::mixin_rule::MixinRule;
pub use crate::ast::sass::statement::return_rule::ReturnRule;
pub use crate::ast::sass::statement::silent_comment::SilentComment;
pub use crate::ast::sass::statement::style_rule::StyleRule;
pub use crate::ast::sass::statement::stylesheet::Stylesheet;
pub use crate::ast::sass::statement::supports_rule::SupportsRule;
pub use crate::ast::sass::statement::use_rule::UseRule;
pub use crate::ast::sass::statement::variable_declaration::VariableDeclaration;
pub use crate::ast::sass::statement::warn_rule::WarnRule;
pub use crate::ast::sass::statement::while_rule::WhileRule;

#[derive(Clone, Debug)]
pub enum Statement<'parse> {
    AtRootRule(AtRootRule<'parse>),
    AtRule(AtRule<'parse>),
    ContentBlock(ContentBlock<'parse>),
    ContentRule(ContentRule<'parse>),
    DebugRule(DebugRule<'parse>),
    Declaration(Declaration<'parse>),
    EachRule(EachRule<'parse>),
    ErrorRule(ErrorRule<'parse>),
    ExtendRule(ExtendRule<'parse>),
    ForRule(ForRule<'parse>),
    ForwardRule(ForwardRule<'parse>),
    FunctionRule(FunctionRule<'parse>),
    IfRule(IfRule<'parse>),
    ImportRule(ImportRule<'parse>),
    IncludeRule(IncludeRule<'parse>),
    LoudComment(LoudComment<'parse>),
    MediaRule(MediaRule<'parse>),
    MixinRule(MixinRule<'parse>),
    ReturnRule(ReturnRule<'parse>),
    SilentComment(SilentComment<'parse>),
    Stylesheet(Stylesheet<'parse>),
    StyleRule(StyleRule<'parse>),
    SupportsRule(SupportsRule<'parse>),
    UseRule(UseRule<'parse>),
    VariableDeclaration(VariableDeclaration<'parse>),
    WarnRule(WarnRule<'parse>),
    WhileRule(WhileRule<'parse>),
}

impl<'parse> Statement<'parse> {
    pub fn accept<V: StatementVisitor<'parse> + ?Sized>(
        &self,
        visitor: &mut V,
    ) -> SassResult<V::Output> {
        match self {
            Statement::AtRootRule(node) => visitor.visit_at_root_rule(node),
            Statement::AtRule(node) => visitor.visit_at_rule(node),
            Statement::ContentBlock(node) => visitor.visit_content_block(node),
            Statement::ContentRule(node) => visitor.visit_content_rule(node),
            Statement::DebugRule(node) => visitor.visit_debug_rule(node),
            Statement::Declaration(node) => visitor.visit_declaration(node),
            Statement::EachRule(node) => visitor.visit_each_rule(node),
            Statement::ErrorRule(node) => visitor.visit_error_rule(node),
            Statement::ExtendRule(node) => visitor.visit_extend_rule(node),
            Statement::ForRule(node) => visitor.visit_for_rule(node),
            Statement::ForwardRule(node) => visitor.visit_forward_rule(node),
            Statement::FunctionRule(node) => visitor.visit_function_rule(node),
            Statement::IfRule(node) => visitor.visit_if_rule(node),
            Statement::ImportRule(node) => visitor.visit_import_rule(node),
            Statement::IncludeRule(node) => visitor.visit_include_rule(node),
            Statement::LoudComment(node) => visitor.visit_loud_comment(node),
            Statement::MediaRule(node) => visitor.visit_media_rule(node),
            Statement::MixinRule(node) => visitor.visit_mixin_rule(node),
            Statement::ReturnRule(node) => visitor.visit_return_rule(node),
            Statement::SilentComment(node) => visitor.visit_silent_comment(node),
            Statement::Stylesheet(node) => visitor.visit_stylesheet(node),
            Statement::StyleRule(node) => visitor.visit_style_rule(node),
            Statement::SupportsRule(node) => visitor.visit_supports_rule(node),
            Statement::UseRule(node) => visitor.visit_use_rule(node),
            Statement::VariableDeclaration(node) => visitor.visit_variable_declaration(node),
            Statement::WarnRule(node) => visitor.visit_warn_rule(node),
            Statement::WhileRule(node) => visitor.visit_while_rule(node),
        }
    }
}

impl<'parse> AstNode<'parse> for Statement<'parse> {
    fn span(&self) -> SassResult<FileSpan<'parse>> {
        match self {
            Statement::AtRootRule(node) => node.span(),
            Statement::AtRule(node) => node.span(),
            Statement::ContentBlock(node) => node.span(),
            Statement::ContentRule(node) => node.span(),
            Statement::DebugRule(node) => node.span(),
            Statement::Declaration(node) => node.span(),
            Statement::EachRule(node) => node.span(),
            Statement::ErrorRule(node) => node.span(),
            Statement::ExtendRule(node) => node.span(),
            Statement::ForRule(node) => node.span(),
            Statement::ForwardRule(node) => node.span(),
            Statement::FunctionRule(node) => node.span(),
            Statement::IfRule(node) => node.span(),
            Statement::ImportRule(node) => node.span(),
            Statement::IncludeRule(node) => node.span(),
            Statement::LoudComment(node) => node.span(),
            Statement::MediaRule(node) => node.span(),
            Statement::MixinRule(node) => node.span(),
            Statement::ReturnRule(node) => node.span(),
            Statement::SilentComment(node) => node.span(),
            Statement::Stylesheet(node) => node.span(),
            Statement::StyleRule(node) => node.span(),
            Statement::SupportsRule(node) => node.span(),
            Statement::UseRule(node) => node.span(),
            Statement::VariableDeclaration(node) => node.span(),
            Statement::WarnRule(node) => node.span(),
            Statement::WhileRule(node) => node.span(),
        }
    }
}

impl<'parse> Statement<'parse> {
    pub fn to_display_string(&self) -> SassResult<String> {
        match self {
            Statement::AtRootRule(node) => node.to_display_string(),
            Statement::AtRule(node) => node.to_display_string(),
            Statement::ContentBlock(node) => node.to_display_string(),
            Statement::ContentRule(node) => node.to_display_string(),
            Statement::DebugRule(node) => node.to_display_string(),
            Statement::Declaration(node) => node.to_display_string(),
            Statement::EachRule(node) => node.to_display_string(),
            Statement::ErrorRule(node) => node.to_display_string(),
            Statement::ExtendRule(node) => node.to_display_string(),
            Statement::ForRule(node) => node.to_display_string(),
            Statement::ForwardRule(node) => node.to_display_string(),
            Statement::FunctionRule(node) => node.to_display_string(),
            Statement::IfRule(node) => node.to_display_string(),
            Statement::ImportRule(node) => node.to_display_string(),
            Statement::IncludeRule(node) => node.to_display_string(),
            Statement::LoudComment(node) => node.to_display_string(),
            Statement::MediaRule(node) => node.to_display_string(),
            Statement::MixinRule(node) => node.to_display_string(),
            Statement::ReturnRule(node) => node.to_display_string(),
            Statement::SilentComment(node) => node.to_display_string(),
            Statement::Stylesheet(node) => node.to_display_string(),
            Statement::StyleRule(node) => node.to_display_string(),
            Statement::SupportsRule(node) => node.to_display_string(),
            Statement::UseRule(node) => node.to_display_string(),
            Statement::VariableDeclaration(node) => node.to_display_string(),
            Statement::WarnRule(node) => node.to_display_string(),
            Statement::WhileRule(node) => node.to_display_string(),
        }
    }
}

impl<'parse> fmt::Display for Statement<'parse> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.to_display_string() {
            Ok(s) => f.write_str(&s),
            Err(_) => Err(fmt::Error),
        }
    }
}

pub trait StatementVisitor<'parse> {
    type Output;

    fn visit_at_root_rule(&mut self, node: &AtRootRule<'parse>) -> SassResult<Self::Output>;
    fn visit_at_rule(&mut self, node: &AtRule<'parse>) -> SassResult<Self::Output>;
    fn visit_content_block(&mut self, node: &ContentBlock<'parse>) -> SassResult<Self::Output>;
    fn visit_content_rule(&mut self, node: &ContentRule<'parse>) -> SassResult<Self::Output>;
    fn visit_debug_rule(&mut self, node: &DebugRule<'parse>) -> SassResult<Self::Output>;
    fn visit_declaration(&mut self, node: &Declaration<'parse>) -> SassResult<Self::Output>;
    fn visit_each_rule(&mut self, node: &EachRule<'parse>) -> SassResult<Self::Output>;
    fn visit_error_rule(&mut self, node: &ErrorRule<'parse>) -> SassResult<Self::Output>;
    fn visit_extend_rule(&mut self, node: &ExtendRule<'parse>) -> SassResult<Self::Output>;
    fn visit_for_rule(&mut self, node: &ForRule<'parse>) -> SassResult<Self::Output>;
    fn visit_forward_rule(&mut self, node: &ForwardRule<'parse>) -> SassResult<Self::Output>;
    fn visit_function_rule(&mut self, node: &FunctionRule<'parse>) -> SassResult<Self::Output>;
    fn visit_if_rule(&mut self, node: &IfRule<'parse>) -> SassResult<Self::Output>;
    fn visit_import_rule(&mut self, node: &ImportRule<'parse>) -> SassResult<Self::Output>;
    fn visit_include_rule(&mut self, node: &IncludeRule<'parse>) -> SassResult<Self::Output>;
    fn visit_loud_comment(&mut self, node: &LoudComment<'parse>) -> SassResult<Self::Output>;
    fn visit_media_rule(&mut self, node: &MediaRule<'parse>) -> SassResult<Self::Output>;
    fn visit_mixin_rule(&mut self, node: &MixinRule<'parse>) -> SassResult<Self::Output>;
    fn visit_return_rule(&mut self, node: &ReturnRule<'parse>) -> SassResult<Self::Output>;
    fn visit_silent_comment(&mut self, node: &SilentComment<'parse>) -> SassResult<Self::Output>;
    fn visit_stylesheet(&mut self, node: &Stylesheet<'parse>) -> SassResult<Self::Output>;
    fn visit_style_rule(&mut self, node: &StyleRule<'parse>) -> SassResult<Self::Output>;
    fn visit_supports_rule(&mut self, node: &SupportsRule<'parse>) -> SassResult<Self::Output>;
    fn visit_use_rule(&mut self, node: &UseRule<'parse>) -> SassResult<Self::Output>;
    fn visit_variable_declaration(
        &mut self,
        node: &VariableDeclaration<'parse>,
    ) -> SassResult<Self::Output>;
    fn visit_warn_rule(&mut self, node: &WarnRule<'parse>) -> SassResult<Self::Output>;
    fn visit_while_rule(&mut self, node: &WhileRule<'parse>) -> SassResult<Self::Output>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::sass::expression::Expression;
    use crate::ast::sass::expression_boolean::BooleanExpression;
    use crate::ast::sass::interpolation::Interpolation;
    use crate::common::source_span_file_source::FileSource;
    use bumpalo::Bump;

    struct MockVisitor {
        visited: String,
    }

    impl<'parse> StatementVisitor<'parse> for MockVisitor {
        type Output = String;

        fn visit_at_root_rule(&mut self, _node: &AtRootRule<'parse>) -> SassResult<String> {
            self.visited = "AtRootRule".into();
            Ok("AtRootRule".into())
        }
        fn visit_at_rule(&mut self, _node: &AtRule<'parse>) -> SassResult<String> {
            self.visited = "AtRule".into();
            Ok("AtRule".into())
        }
        fn visit_content_block(&mut self, _node: &ContentBlock<'parse>) -> SassResult<String> {
            self.visited = "ContentBlock".into();
            Ok("ContentBlock".into())
        }
        fn visit_content_rule(&mut self, _node: &ContentRule<'parse>) -> SassResult<String> {
            self.visited = "ContentRule".into();
            Ok("ContentRule".into())
        }
        fn visit_debug_rule(&mut self, _node: &DebugRule<'parse>) -> SassResult<String> {
            self.visited = "DebugRule".into();
            Ok("DebugRule".into())
        }
        fn visit_declaration(&mut self, _node: &Declaration<'parse>) -> SassResult<String> {
            self.visited = "Declaration".into();
            Ok("Declaration".into())
        }
        fn visit_each_rule(&mut self, _node: &EachRule<'parse>) -> SassResult<String> {
            self.visited = "EachRule".into();
            Ok("EachRule".into())
        }
        fn visit_error_rule(&mut self, _node: &ErrorRule<'parse>) -> SassResult<String> {
            self.visited = "ErrorRule".into();
            Ok("ErrorRule".into())
        }
        fn visit_extend_rule(&mut self, _node: &ExtendRule<'parse>) -> SassResult<String> {
            self.visited = "ExtendRule".into();
            Ok("ExtendRule".into())
        }
        fn visit_for_rule(&mut self, _node: &ForRule<'parse>) -> SassResult<String> {
            self.visited = "ForRule".into();
            Ok("ForRule".into())
        }
        fn visit_forward_rule(&mut self, _node: &ForwardRule<'parse>) -> SassResult<String> {
            self.visited = "ForwardRule".into();
            Ok("ForwardRule".into())
        }
        fn visit_function_rule(&mut self, _node: &FunctionRule<'parse>) -> SassResult<String> {
            self.visited = "FunctionRule".into();
            Ok("FunctionRule".into())
        }
        fn visit_if_rule(&mut self, _node: &IfRule<'parse>) -> SassResult<String> {
            self.visited = "IfRule".into();
            Ok("IfRule".into())
        }
        fn visit_import_rule(&mut self, _node: &ImportRule<'parse>) -> SassResult<String> {
            self.visited = "ImportRule".into();
            Ok("ImportRule".into())
        }
        fn visit_include_rule(&mut self, _node: &IncludeRule<'parse>) -> SassResult<String> {
            self.visited = "IncludeRule".into();
            Ok("IncludeRule".into())
        }
        fn visit_loud_comment(&mut self, _node: &LoudComment<'parse>) -> SassResult<String> {
            self.visited = "LoudComment".into();
            Ok("LoudComment".into())
        }
        fn visit_media_rule(&mut self, _node: &MediaRule<'parse>) -> SassResult<String> {
            self.visited = "MediaRule".into();
            Ok("MediaRule".into())
        }
        fn visit_mixin_rule(&mut self, _node: &MixinRule<'parse>) -> SassResult<String> {
            self.visited = "MixinRule".into();
            Ok("MixinRule".into())
        }
        fn visit_return_rule(&mut self, _node: &ReturnRule<'parse>) -> SassResult<String> {
            self.visited = "ReturnRule".into();
            Ok("ReturnRule".into())
        }
        fn visit_silent_comment(&mut self, _node: &SilentComment<'parse>) -> SassResult<String> {
            self.visited = "SilentComment".into();
            Ok("SilentComment".into())
        }
        fn visit_stylesheet(&mut self, _node: &Stylesheet<'parse>) -> SassResult<String> {
            self.visited = "Stylesheet".into();
            Ok("Stylesheet".into())
        }
        fn visit_style_rule(&mut self, _node: &StyleRule<'parse>) -> SassResult<String> {
            self.visited = "StyleRule".into();
            Ok("StyleRule".into())
        }
        fn visit_supports_rule(&mut self, _node: &SupportsRule<'parse>) -> SassResult<String> {
            self.visited = "SupportsRule".into();
            Ok("SupportsRule".into())
        }
        fn visit_use_rule(&mut self, _node: &UseRule<'parse>) -> SassResult<String> {
            self.visited = "UseRule".into();
            Ok("UseRule".into())
        }
        fn visit_variable_declaration(
            &mut self,
            _node: &VariableDeclaration<'parse>,
        ) -> SassResult<String> {
            self.visited = "VariableDeclaration".into();
            Ok("VariableDeclaration".into())
        }
        fn visit_warn_rule(&mut self, _node: &WarnRule<'parse>) -> SassResult<String> {
            self.visited = "WarnRule".into();
            Ok("WarnRule".into())
        }
        fn visit_while_rule(&mut self, _node: &WhileRule<'parse>) -> SassResult<String> {
            self.visited = "WhileRule".into();
            Ok("WhileRule".into())
        }
    }

    fn test_span<'compile, 'parse>(arena: &'compile Bump, text: &str) -> FileSpan<'parse>
    where
        'compile: 'parse,
        'parse: 'compile,
    {
        let fs = FileSource::new_in(arena, text, None);
        FileSpan::new(Some(fs), 0, text.len())
    }

    #[test]
    fn test_accept_dispatch_at_root_rule() {
        let arena = Bump::new();
        let span = test_span(&arena, "@at-root");
        let rule = AtRootRule::new(vec![], span, None);
        let stmt = Statement::AtRootRule(rule);
        let mut visitor = MockVisitor {
            visited: String::new(),
        };
        let result = stmt.accept(&mut visitor).unwrap();
        assert_eq!(result, "AtRootRule");
        assert_eq!(visitor.visited, "AtRootRule");
    }

    #[test]
    fn test_accept_dispatch_extend_rule() {
        let arena = Bump::new();
        let span = test_span(&arena, "@extend .foo;");
        let interp = Interpolation::plain(".foo".into(), span);
        let rule = ExtendRule::new(interp, span, false);
        let stmt = Statement::ExtendRule(rule);
        let mut visitor = MockVisitor {
            visited: String::new(),
        };
        let result = stmt.accept(&mut visitor).unwrap();
        assert_eq!(result, "ExtendRule");
        assert_eq!(visitor.visited, "ExtendRule");
    }

    #[test]
    fn test_accept_dispatch_declaration() {
        let arena = Bump::new();
        let span = test_span(&arena, "color: red;");
        let name = Interpolation::plain("color".into(), span);
        let val = Expression::Boolean(BooleanExpression::new(true, span));
        let rule = Declaration::new(name, val, span);
        let stmt = Statement::Declaration(rule);
        let mut visitor = MockVisitor {
            visited: String::new(),
        };
        let result = stmt.accept(&mut visitor).unwrap();
        assert_eq!(result, "Declaration");
        assert_eq!(visitor.visited, "Declaration");
    }

    #[test]
    fn test_display_debug() {
        let arena = Bump::new();
        let span = test_span(&arena, "@debug true;");
        let expr = Expression::Boolean(BooleanExpression::new(true, span));
        let rule = DebugRule::new(expr, span);
        let stmt = Statement::DebugRule(rule);
        assert_eq!(format!("{stmt}"), "@debug true;");
    }
}
