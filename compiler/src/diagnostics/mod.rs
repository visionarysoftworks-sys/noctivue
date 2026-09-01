//! Structured diagnostics shared by all compiler stages.
//!
//! Every stage MUST emit diagnostics through this module rather than writing to stderr
//! directly, so that `noct diagnostics --json` can surface them machine-readably
//! (AI_TOOLING.md §2, COMPILER_ARCHITECTURE.md §6).

/// Severity level of a diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Note,
    Help,
}

/// A half-open byte range `[start, end)` within a source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

/// A single label attached to a span in a diagnostic.
#[derive(Debug, Clone)]
pub struct Label {
    pub span: Span,
    pub message: String,
}

/// A structured compiler diagnostic.
///
/// Diagnostics are emitted by every stage and collected in a [`DiagnosticSink`].
/// They serialize to JSON for `noct diagnostics --json`.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Short, human-readable summary of the problem.
    pub message: String,
    /// Source labels (primary and secondary spans).
    pub labels: Vec<Label>,
    /// Optional longer explanation or suggested fix.
    pub notes: Vec<String>,
    /// Stable error code, e.g. `"E0001"`.
    pub code: Option<String>,
}

impl Diagnostic {
    pub fn error(message: impl Into<String>) -> Self {
        Diagnostic {
            severity: Severity::Error,
            message: message.into(),
            labels: Vec::new(),
            notes: Vec::new(),
            code: None,
        }
    }

    pub fn warning(message: impl Into<String>) -> Self {
        Diagnostic {
            severity: Severity::Warning,
            message: message.into(),
            labels: Vec::new(),
            notes: Vec::new(),
            code: None,
        }
    }

    pub fn with_span(mut self, span: Span, label: impl Into<String>) -> Self {
        self.labels.push(Label { span, message: label.into() });
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

/// Collects diagnostics from all pipeline stages.
#[derive(Debug, Default)]
pub struct DiagnosticSink {
    diagnostics: Vec<Diagnostic>,
}

impl DiagnosticSink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn emit(&mut self, diag: Diagnostic) {
        self.diagnostics.push(diag);
    }

    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(|d| d.is_error())
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn take(self) -> Vec<Diagnostic> {
        self.diagnostics
    }
}
