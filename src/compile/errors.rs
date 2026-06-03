use crate::span::Span;
use ariadne::{Color, Label, Report, ReportKind, sources};
use rustc_hash::FxHashMap as HashMap;

pub type Sources = HashMap<usize, (String, String)>;

/// How much confidence a suggested fix carries. Mirrors the distinction `pit
/// fix` needs: only `MachineApplicable` rewrites are applied automatically,
/// `MaybeIncorrect` ones are shown to the programmer but never written to disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applicability {
    MachineApplicable,
    MaybeIncorrect,
}

/// A concrete source edit that resolves a diagnostic: replace the text covered
/// by `span` with `replacement`. Carries the human-facing message and an
/// applicability so the renderer and the autofixer can treat it correctly.
#[derive(Clone, Debug)]
pub struct Suggestion {
    pub span: Span,
    pub replacement: String,
    pub message: String,
    pub applicability: Applicability,
}

/// A structured compiler diagnostic. A diagnostic carries a stable code, a
/// headline message anchored at a primary span, and any number of secondary
/// labels, explanatory notes, and actionable help lines. This is what lets the
/// renderer point at the offending code, explain *why* it is wrong, and tell
/// the programmer how to fix it.
#[derive(Clone, Debug)]
pub struct Diagnostic {
    code: Option<String>,
    is_error: bool,
    message: String,
    primary: Span,
    primary_label: Option<String>,
    secondary: Vec<(Span, String)>,
    notes: Vec<String>,
    helps: Vec<String>,
    suggestions: Vec<Suggestion>,
}

impl Diagnostic {
    pub fn error(code: &str, message: impl Into<String>, primary: Span) -> Self {
        Self {
            code: (!code.is_empty()).then(|| code.to_string()),
            is_error: true,
            message: message.into(),
            primary,
            primary_label: None,
            secondary: Vec::new(),
            notes: Vec::new(),
            helps: Vec::new(),
            suggestions: Vec::new(),
        }
    }

    /// Message rendered directly under the caret on the primary span.
    pub fn label(mut self, msg: impl Into<String>) -> Self {
        self.primary_label = Some(msg.into());
        self
    }

    /// A related location worth pointing at (e.g. an earlier definition).
    pub fn secondary(mut self, span: Span, msg: impl Into<String>) -> Self {
        self.secondary.push((span, msg.into()));
        self
    }

    /// Background explaining why the code is wrong.
    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    /// A concrete, actionable fix.
    pub fn help(mut self, help: impl Into<String>) -> Self {
        self.helps.push(help.into());
        self
    }

    /// Attach a structured, machine-applicable fix: replace the text under
    /// `span` with `replacement`. Rendered as a code suggestion and applied
    /// verbatim by `pit fix`.
    pub fn fix(
        self,
        span: Span,
        replacement: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        self.suggestion(span, replacement, message, Applicability::MachineApplicable)
    }

    /// Attach a structured fix at a chosen confidence level. `MaybeIncorrect`
    /// suggestions are displayed but never written to disk by the autofixer.
    pub fn suggestion(
        mut self,
        span: Span,
        replacement: impl Into<String>,
        message: impl Into<String>,
        applicability: Applicability,
    ) -> Self {
        self.suggestions.push(Suggestion {
            span,
            replacement: replacement.into(),
            message: message.into(),
            applicability,
        });
        self
    }

    /// The structured fixes attached to this diagnostic, in declaration order.
    pub fn suggestions(&self) -> &[Suggestion] {
        &self.suggestions
    }

    /// Demote an error-level diagnostic to a warning, preserving its labels.
    pub fn into_warning(mut self) -> Self {
        self.is_error = false;
        self
    }

    /// `did you mean` help listing the nearest in-scope names, ordered nearest
    /// first. One name renders inline; several are joined Oxford-style. An empty
    /// list leaves the diagnostic unchanged.
    pub fn suggest_names(self, names: &[String]) -> Self {
        if names.is_empty() {
            self
        } else {
            self.help(format!("did you mean {}?", oxford_join(names)))
        }
    }

    /// The span the headline is anchored at.
    pub fn primary_span(&self) -> Span {
        self.primary
    }

    /// The headline message, without code or labels.
    pub fn headline(&self) -> &str {
        &self.message
    }

    /// The stable diagnostic code, if one was assigned.
    pub fn code(&self) -> Option<&str> {
        self.code.as_deref()
    }

    /// Error severity vs. warning.
    pub fn is_error(&self) -> bool {
        self.is_error
    }

    /// Message rendered directly under the caret on the primary span, if set.
    pub fn primary_label(&self) -> Option<&str> {
        self.primary_label.as_deref()
    }

    /// Related locations worth pointing at, in declaration order.
    pub fn secondary_labels(&self) -> &[(Span, String)] {
        &self.secondary
    }

    /// Background notes explaining why the code is wrong.
    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    /// Actionable help lines.
    pub fn helps(&self) -> &[String] {
        &self.helps
    }

    pub fn emit(&self, src: &Sources) {
        let name = |span: Span| {
            src.get(&span.file_id)
                .map(|(n, _)| n.clone())
                .unwrap_or_else(|| "<unknown>".to_string())
        };

        let kind = if self.is_error {
            ReportKind::Error
        } else {
            ReportKind::Warning
        };
        let accent = if self.is_error {
            Color::Red
        } else {
            Color::Yellow
        };

        let pname = name(self.primary);
        let mut builder =
            Report::build(kind, (pname.clone(), self.primary.start..self.primary.end))
                .with_message(&self.message);

        if let Some(code) = &self.code {
            builder = builder.with_code(code);
        }

        let mut primary =
            Label::new((pname, self.primary.start..self.primary.end)).with_color(accent);
        if let Some(text) = &self.primary_label {
            primary = primary.with_message(text);
        }
        builder = builder.with_label(primary);

        for (span, text) in &self.secondary {
            builder = builder.with_label(
                Label::new((name(*span), span.start..span.end))
                    .with_message(text)
                    .with_color(Color::Cyan)
                    .with_order(1),
            );
        }

        for note in &self.notes {
            builder = builder.with_note(note);
        }
        for help in &self.helps {
            builder = builder.with_help(help);
        }
        for sug in &self.suggestions {
            builder = builder.with_help(render_suggestion(sug));
        }
        if let Some(code) = &self.code {
            builder = builder.with_help(format!(
                "run `pit explain {code}` for a detailed explanation"
            ));
        }

        let cache = sources(
            src.values()
                .map(|(n, s)| (n.clone(), s.clone()))
                .collect::<Vec<_>>(),
        );
        let _ = builder.finish().eprint(cache);
    }
}

/// Joins backticked names into an English list: `a`, `a` or `b`, `a`, `b`, or `c`.
fn oxford_join(names: &[String]) -> String {
    let quoted: Vec<String> = names.iter().map(|n| format!("`{n}`")).collect();
    match quoted.as_slice() {
        [] => String::new(),
        [a] => a.clone(),
        [a, b] => format!("{a} or {b}"),
        [rest @ .., last] => format!("{}, or {last}", rest.join(", ")),
    }
}

/// Formats a structured fix as a help line. A single-line replacement is shown
/// inline; a multi-line one is broken onto its own indented lines so it still
/// reads as the code the programmer should end up with.
fn render_suggestion(sug: &Suggestion) -> String {
    let tail = if sug.applicability == Applicability::MaybeIncorrect {
        " (review before applying)"
    } else {
        ""
    };
    if sug.replacement.contains('\n') {
        let body: String = sug
            .replacement
            .lines()
            .map(|l| format!("\n      {l}"))
            .collect();
        format!("{}{tail}:{body}", sug.message)
    } else if sug.replacement.is_empty() {
        format!("{}{tail}", sug.message)
    } else {
        format!("{}{tail}: `{}`", sug.message, sug.replacement)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::Span;

    fn span(file_id: usize, start: usize, end: usize) -> Span {
        Span {
            file_id,
            line: 1,
            col: 1,
            start,
            end,
        }
    }

    fn one_source() -> Sources {
        let mut s = Sources::default();
        s.insert(0, ("test.liv".to_string(), "let x = 42\n".to_string()));
        s
    }

    #[test]
    fn error_with_valid_sources() {
        Diagnostic::error("", "test error message", span(0, 0, 10)).emit(&one_source());
    }

    #[test]
    fn error_with_multiple_sources() {
        let mut sources = Sources::default();
        sources.insert(0, ("a.liv".to_string(), "content a".to_string()));
        sources.insert(1, ("b.liv".to_string(), "content b".to_string()));
        Diagnostic::error("", "error in a", span(0, 0, 5)).emit(&sources);
        Diagnostic::error("", "error in b", span(1, 0, 5)).emit(&sources);
    }

    #[test]
    fn error_zero_length_span() {
        let mut sources = Sources::default();
        sources.insert(0, ("test.liv".to_string(), "abc".to_string()));
        Diagnostic::error("", "zero-length span error", span(0, 2, 2)).emit(&sources);
    }

    #[test]
    fn full_diagnostic_renders() {
        let mut sources = Sources::default();
        sources.insert(
            0,
            (
                "m.liv".to_string(),
                "let total = 1\nprint(totl)\n".to_string(),
            ),
        );
        Diagnostic::error("E0001", "undefined name `totl`", span(0, 20, 24))
            .label("not found in this scope")
            .secondary(span(0, 4, 9), "a similar name is defined here")
            .note("names must be bound with `let` before use")
            .suggest_names(&["total".to_string()])
            .emit(&sources);
    }

    #[test]
    fn oxford_join_forms() {
        let s = |v: &[&str]| oxford_join(&v.iter().map(|x| x.to_string()).collect::<Vec<_>>());
        assert_eq!(s(&[]), "");
        assert_eq!(s(&["a"]), "`a`");
        assert_eq!(s(&["a", "b"]), "`a` or `b`");
        assert_eq!(s(&["a", "b", "c"]), "`a`, `b`, or `c`");
    }

    #[test]
    fn missing_file_id_falls_back() {
        Diagnostic::error("E9999", "orphan span", span(7, 0, 1)).emit(&one_source());
    }
}
