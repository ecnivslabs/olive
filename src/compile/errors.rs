use crate::span;
use ariadne::{Label, Report, ReportKind, Source};
use rustc_hash::FxHashMap as HashMap;

pub fn report_error(sources: &HashMap<usize, (String, String)>, msg: &str, span: span::Span) {
    report(sources, ReportKind::Error, msg, span);
}

pub fn report_warning(sources: &HashMap<usize, (String, String)>, msg: &str, span: span::Span) {
    report(sources, ReportKind::Warning, msg, span);
}

fn report(
    sources: &HashMap<usize, (String, String)>,
    kind: ReportKind,
    msg: &str,
    span: span::Span,
) {
    let (filename, source) = sources
        .get(&span.file_id)
        .expect("file not found in sources");
    let _ = Report::build(kind, (filename.as_str(), span.start..span.end))
        .with_message(msg)
        .with_label(Label::new((filename.as_str(), span.start..span.end)))
        .finish()
        .print((filename.as_str(), Source::from(source)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::Span;
    use rustc_hash::FxHashMap as HashMap;

    #[test]
    fn report_error_with_valid_sources() {
        let mut sources = HashMap::default();
        sources.insert(0, ("test.liv".to_string(), "let x = 42\n".to_string()));
        let span = Span {
            file_id: 0,
            line: 1,
            col: 1,
            start: 0,
            end: 10,
        };
        report_error(&sources, "test error message", span);
    }

    #[test]
    fn report_error_with_multiple_sources() {
        let mut sources = HashMap::default();
        sources.insert(0, ("a.liv".to_string(), "content a".to_string()));
        sources.insert(1, ("b.liv".to_string(), "content b".to_string()));
        report_error(
            &sources,
            "error in a",
            Span {
                file_id: 0,
                line: 1,
                col: 1,
                start: 0,
                end: 5,
            },
        );
        report_error(
            &sources,
            "error in b",
            Span {
                file_id: 1,
                line: 1,
                col: 1,
                start: 0,
                end: 5,
            },
        );
    }

    #[test]
    fn report_error_zero_length_span() {
        let mut sources = HashMap::default();
        sources.insert(0, ("test.liv".to_string(), "abc".to_string()));
        let span = Span {
            file_id: 0,
            line: 1,
            col: 2,
            start: 2,
            end: 2,
        };
        report_error(&sources, "zero-length span error", span);
    }
}
