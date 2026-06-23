use std::rc::Rc;

/// Indentation step, in spaces. Olive sources are 4-space indented.
pub const INDENT: usize = 4;

/// A document in the Wadler/Leijen pretty-printing algebra. A `Doc` describes the
/// *structure* of the output (where breaks may happen and how deep to indent), and
/// the renderer decides flat-vs-broken per `Group` against a width budget.
#[derive(Clone)]
pub enum Doc {
    Nil,
    Text(Rc<str>),
    /// Space when its group is flat, newline when broken.
    Line,
    /// Nothing when flat, newline when broken.
    SoftLine,
    /// Always a newline. A group containing one can never lay flat.
    HardLine,
    Concat(Rc<Doc>, Rc<Doc>),
    Nest(usize, Rc<Doc>),
    Group(Rc<Doc>),
    /// `IfBreak(broken, flat)` renders `broken` when the enclosing group breaks and
    /// `flat` when it lays flat. Used for trailing commas.
    IfBreak(Rc<Doc>, Rc<Doc>),
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Flat,
    Break,
}

pub fn nil() -> Doc {
    Doc::Nil
}

pub fn text(s: impl Into<String>) -> Doc {
    Doc::Text(Rc::from(s.into().as_str()))
}

pub fn line() -> Doc {
    Doc::Line
}

pub fn softline() -> Doc {
    Doc::SoftLine
}

pub fn hardline() -> Doc {
    Doc::HardLine
}

pub fn nest(n: usize, d: Doc) -> Doc {
    Doc::Nest(n, Rc::new(d))
}

pub fn group(d: Doc) -> Doc {
    Doc::Group(Rc::new(d))
}

pub fn if_break(broken: Doc, flat: Doc) -> Doc {
    Doc::IfBreak(Rc::new(broken), Rc::new(flat))
}

pub fn concat(a: Doc, b: Doc) -> Doc {
    match (&a, &b) {
        (Doc::Nil, _) => b,
        (_, Doc::Nil) => a,
        _ => Doc::Concat(Rc::new(a), Rc::new(b)),
    }
}

/// Concatenate a sequence of docs with no separator.
pub fn concat_all(docs: impl IntoIterator<Item = Doc>) -> Doc {
    docs.into_iter().fold(Doc::Nil, concat)
}

/// Join docs with `sep` between each adjacent pair.
pub fn join(sep: Doc, docs: impl IntoIterator<Item = Doc>) -> Doc {
    let mut out = Doc::Nil;
    for (i, d) in docs.into_iter().enumerate() {
        if i > 0 {
            out = concat(out, sep.clone());
        }
        out = concat(out, d);
    }
    out
}

/// A delimited, breakable list: `open`, the comma-separated `items`, then `close`.
/// Flat: `open a, b close`. Broken: each item on its own indented line with a
/// trailing comma. The breaks live *inside* the delimiters, where Olive's lexer
/// ignores newlines, so wrapping here never changes tokenization.
pub fn bracketed(open: &str, items: Vec<Doc>, close: &str) -> Doc {
    if items.is_empty() {
        return concat(text(open), text(close));
    }
    let sep = concat(text(","), line());
    let inner = concat(softline(), join(sep, items));
    group(concat_all([
        text(open),
        nest(INDENT, concat(inner, if_break(text(","), nil()))),
        softline(),
        text(close),
    ]))
}

/// Render `doc` to a string, breaking groups that would exceed `width` columns.
pub fn render(doc: &Doc, width: usize) -> String {
    let mut out = String::new();
    let mut col: usize = 0;
    let mut stack: Vec<(usize, Mode, Rc<Doc>)> = vec![(0, Mode::Break, Rc::new(doc.clone()))];

    while let Some((indent, mode, d)) = stack.pop() {
        match &*d {
            Doc::Nil => {}
            Doc::Text(s) => {
                out.push_str(s);
                col += s.chars().count();
            }
            Doc::Concat(a, b) => {
                stack.push((indent, mode, b.clone()));
                stack.push((indent, mode, a.clone()));
            }
            Doc::Nest(n, inner) => stack.push((indent + n, mode, inner.clone())),
            Doc::Line => match mode {
                Mode::Flat => {
                    out.push(' ');
                    col += 1;
                }
                Mode::Break => newline(&mut out, &mut col, indent),
            },
            Doc::SoftLine => {
                if mode == Mode::Break {
                    newline(&mut out, &mut col, indent);
                }
            }
            Doc::HardLine => newline(&mut out, &mut col, indent),
            Doc::IfBreak(broken, flat) => {
                let chosen = if mode == Mode::Break { broken } else { flat };
                stack.push((indent, mode, chosen.clone()));
            }
            Doc::Group(inner) => {
                let remaining = width as isize - col as isize;
                let group_mode = if fits(remaining, (indent, Mode::Flat, inner.clone()), &stack) {
                    Mode::Flat
                } else {
                    Mode::Break
                };
                stack.push((indent, group_mode, inner.clone()));
            }
        }
    }
    out
}

fn newline(out: &mut String, col: &mut usize, indent: usize) {
    out.push('\n');
    for _ in 0..indent {
        out.push(' ');
    }
    *col = indent;
}

/// Does the candidate (`first`, then the pending `rest`) fit flat in `remaining`
/// columns before the next forced break? `rest` is the renderer's continuation, so a
/// group is judged against what follows it, not just its own content.
fn fits(
    mut remaining: isize,
    first: (usize, Mode, Rc<Doc>),
    rest: &[(usize, Mode, Rc<Doc>)],
) -> bool {
    let mut local: Vec<(usize, Mode, Rc<Doc>)> = vec![first];
    let mut rest_idx = rest.len();

    loop {
        if remaining < 0 {
            return false;
        }
        let (indent, mode, d) = match local.pop() {
            Some(x) => x,
            None => {
                if rest_idx == 0 {
                    return true;
                }
                rest_idx -= 1;
                rest[rest_idx].clone()
            }
        };
        match &*d {
            Doc::Nil => {}
            Doc::Text(s) => remaining -= s.chars().count() as isize,
            Doc::Concat(a, b) => {
                local.push((indent, mode, b.clone()));
                local.push((indent, mode, a.clone()));
            }
            Doc::Nest(n, inner) => local.push((indent + n, mode, inner.clone())),
            Doc::Group(inner) => local.push((indent, Mode::Flat, inner.clone())),
            Doc::Line => match mode {
                Mode::Flat => remaining -= 1,
                Mode::Break => return true,
            },
            Doc::SoftLine => {
                if mode == Mode::Break {
                    return true;
                }
            }
            Doc::HardLine => match mode {
                Mode::Flat => return false,
                Mode::Break => return true,
            },
            Doc::IfBreak(broken, flat) => {
                let chosen = if mode == Mode::Break { broken } else { flat };
                local.push((indent, mode, chosen.clone()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(n: usize) -> Doc {
        let items: Vec<Doc> = (0..n).map(|i| text(format!("item{i}"))).collect();
        bracketed("[", items, "]")
    }

    #[test]
    fn flat_when_it_fits() {
        let d = list(3);
        assert_eq!(render(&d, 80), "[item0, item1, item2]");
    }

    #[test]
    fn breaks_when_too_wide() {
        let d = list(3);
        let out = render(&d, 10);
        assert_eq!(out, "[\n    item0,\n    item1,\n    item2,\n]");
    }

    #[test]
    fn empty_bracket_stays_flat() {
        let d = bracketed("(", vec![], ")");
        assert_eq!(render(&d, 80), "()");
    }

    #[test]
    fn nested_groups_break_independently() {
        let inner = list(2);
        let outer = bracketed("f(", vec![inner, text("tail")], ")");
        // Outer must break, inner still fits flat.
        let out = render(&outer, 20);
        assert_eq!(out, "f(\n    [item0, item1],\n    tail,\n)");
    }

    #[test]
    fn hardline_forces_break() {
        let d = group(concat_all([text("a"), hardline(), text("b")]));
        assert_eq!(render(&d, 80), "a\nb");
    }

    #[test]
    fn nest_indents_after_break() {
        let d = group(concat_all([
            text("x"),
            nest(INDENT, concat(line(), text("y"))),
        ]));
        assert_eq!(render(&d, 2), "x\n    y");
    }
}
