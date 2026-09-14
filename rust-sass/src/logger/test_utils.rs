// Test-only recording logger shared across test modules.
// go-source: (test helper) go/functions/map_test.go recordingLogger

use std::cell::RefCell;

use crate::common::exception::{SassError, SassResult, Trace};
use crate::common::span::Span;
use crate::deprecation::Deprecation;

use crate::logger::Logger;

/// A single recorded logger call. Rust-only shape (Dart/Go record plain
/// message strings); `has_span`/`trace` are kept so tests can assert
/// span/trace routing without retaining arena spans.
#[derive(Debug, Clone)]
pub enum LogCall {
    Warn {
        msg: String,
        has_span: bool,
        trace: Option<Trace>,
    },
    Debug {
        msg: String,
        has_span: bool,
    },
    WarnDeprecation {
        msg: String,
        has_span: bool,
        dep_id: String,
        trace: Option<Trace>,
    },
}

/// Records warn / debug / warn_deprecation calls for test assertions.
///
/// Test-only helper with no Dart counterpart (Dart tests use real loggers or
/// mocks inline). Mirrors Go's test `recordingLogger` in
/// `go/functions/map_test.go` (warn + deprecation messages recorded in
/// order). `RefCell` is fine here: the compiler is single-threaded and
/// loggers are shared via `Rc<dyn Logger>` on one thread.
#[derive(Debug, Default)]
pub struct RecordLogger {
    calls: RefCell<Vec<LogCall>>,
    deprecation_error: RefCell<Option<Box<SassError>>>,
}

impl RecordLogger {
    pub fn new() -> Self {
        RecordLogger::default()
    }

    /// All recorded calls, in order. Rust-only (Go exposes only `messages` /
    /// `deprecations` projections).
    pub fn calls(&self) -> Vec<LogCall> {
        self.calls.borrow().clone()
    }

    /// Warn + WarnDeprecation messages in order (Debug excluded), matching
    /// Go's recordingLogger `messages` field.
    pub fn messages(&self) -> Vec<String> {
        self.calls
            .borrow()
            .iter()
            .filter_map(|c| match c {
                LogCall::Warn { msg, .. } | LogCall::WarnDeprecation { msg, .. } => {
                    Some(msg.clone())
                }
                LogCall::Debug { .. } => None,
            })
            .collect()
    }

    /// Deprecation IDs aligned with `messages()`: `Some(id)` for
    /// WarnDeprecation, `None` for a plain Warn. Matches Go's recordingLogger
    /// `deprecations` field (nil for plain warnings).
    pub fn deprecation_ids(&self) -> Vec<Option<String>> {
        self.calls
            .borrow()
            .iter()
            .filter_map(|c| match c {
                LogCall::Warn { .. } => Some(None),
                LogCall::WarnDeprecation { dep_id, .. } => Some(Some(dep_id.clone())),
                LogCall::Debug { .. } => None,
            })
            .collect()
    }

    /// Makes the next `warn_deprecation` call return this error (consumed
    /// once). Rust-only fault-injection seam; Go's `recordingLogger` always
    /// returns nil.
    pub fn set_deprecation_error(&self, err: Box<SassError>) {
        self.deprecation_error.borrow_mut().replace(err);
    }
}

impl Logger for RecordLogger {
    fn warn<'a>(&self, message: &str, span: Option<&Span<'a>>, trace: Option<&Trace>) {
        self.calls.borrow_mut().push(LogCall::Warn {
            msg: message.to_string(),
            has_span: span.is_some(),
            trace: trace.cloned(),
        });
    }

    fn debug<'a>(&self, message: &str, span: Option<&Span<'a>>) {
        self.calls.borrow_mut().push(LogCall::Debug {
            msg: message.to_string(),
            has_span: span.is_some(),
        });
    }

    fn warn_deprecation<'a>(
        &self,
        message: &str,
        span: Option<&Span<'a>>,
        deprecation: &'static Deprecation,
        trace: Option<&Trace>,
    ) -> SassResult<()> {
        self.calls.borrow_mut().push(LogCall::WarnDeprecation {
            msg: message.to_string(),
            has_span: span.is_some(),
            dep_id: deprecation.id.to_string(),
            trace: trace.cloned(),
        });
        if let Some(err) = self.deprecation_error.borrow_mut().take() {
            return Err(err);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::exception::Frame;
    use crate::deprecation::CALL_STRING;

    #[test]
    fn test_record_logger_records_calls_in_order() {
        let rl = RecordLogger::new();
        let trace = Trace::new(vec![Frame {
            uri: None,
            line: 1,
            column: 1,
            member: "stack".to_string(),
        }]);
        rl.warn("warn msg", None, Some(&trace));
        rl.debug("debug msg", None);
        rl.warn_deprecation("dep msg", None, &CALL_STRING, None)
            .unwrap();

        let calls = rl.calls();
        assert_eq!(calls.len(), 3);
        match &calls[0] {
            LogCall::Warn {
                msg,
                has_span,
                trace,
            } => {
                assert_eq!(msg, "warn msg");
                assert!(!has_span);
                assert_eq!(trace.as_ref().unwrap()[0].member, "stack");
            }
            other => panic!("expected Warn, got {other:?}"),
        }
        match &calls[1] {
            LogCall::Debug { msg, .. } => assert_eq!(msg, "debug msg"),
            other => panic!("expected Debug, got {other:?}"),
        }
        match &calls[2] {
            LogCall::WarnDeprecation { msg, dep_id, .. } => {
                assert_eq!(msg, "dep msg");
                assert_eq!(dep_id, "call-string");
            }
            other => panic!("expected WarnDeprecation, got {other:?}"),
        }
    }

    #[test]
    fn test_record_logger_messages_excludes_debug() {
        let rl = RecordLogger::new();
        rl.warn("a", None, None);
        rl.debug("skipped", None);
        rl.warn_deprecation("b", None, &CALL_STRING, None).unwrap();

        assert_eq!(rl.messages(), vec!["a".to_string(), "b".to_string()]);
        assert_eq!(
            rl.deprecation_ids(),
            vec![None, Some("call-string".to_string())]
        );
    }

    #[test]
    fn test_record_logger_deprecation_error() {
        let rl = RecordLogger::new();
        rl.set_deprecation_error(Box::new(SassError::Script {
            message: "injected".into(),
            argument_name: None,
        }));
        let err = rl
            .warn_deprecation("dep", None, &CALL_STRING, None)
            .unwrap_err();
        assert_eq!(err.message(), "injected");
        // The error is consumed; the next call succeeds.
        rl.warn_deprecation("dep2", None, &CALL_STRING, None)
            .unwrap();
        assert_eq!(rl.messages().len(), 2);
    }
}
