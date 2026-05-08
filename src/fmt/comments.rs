use crate::lexer::{Comment, CommentKind};

/// Tracks comments recovered by the lexer and hands them back, in source order, as
/// statements are lowered. Placement is span-driven: a comment is *standalone* when it
/// sits on its own line before a statement, and *trailing* when it shares a line with
/// the code that precedes it. Blank-line runs are collapsed to a single blank by the
/// caller using the line numbers this type exposes.
pub struct CommentWeaver {
    comments: Vec<Comment>,
    idx: usize,
    line_starts: Vec<usize>,
}

impl CommentWeaver {
    pub fn new(source: &[char], comments: &[Comment]) -> Self {
        let mut line_starts = vec![0usize];
        for (i, &c) in source.iter().enumerate() {
            if c == '\n' {
                line_starts.push(i + 1);
            }
        }
        let mut comments = comments.to_vec();
        comments.sort_by_key(|c| c.span.0);
        Self {
            comments,
            idx: 0,
            line_starts,
        }
    }

    /// 0-based line index for a char offset.
    pub fn line_of(&self, offset: usize) -> usize {
        match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        }
    }

    /// 0-based column (indentation) of a char offset.
    pub fn column_of(&self, offset: usize) -> usize {
        offset - self.line_starts[self.line_of(offset)]
    }

    /// Next comment if it starts before `offset` and is indented at least `min_col`,
    /// consuming it. A shallower comment blocks the take so it stays with the outer
    /// scope it visually belongs to.
    pub fn take_before_col(&mut self, offset: usize, min_col: usize) -> Option<Comment> {
        let hit = self
            .peek()
            .is_some_and(|c| c.span.0 < offset && self.column_of(c.span.0) >= min_col);
        if hit {
            let c = self.comments[self.idx].clone();
            self.idx += 1;
            Some(c)
        } else {
            None
        }
    }

    fn peek(&self) -> Option<&Comment> {
        self.comments.get(self.idx)
    }

    /// Next comment if it starts before `offset`, consuming it.
    pub fn take_before(&mut self, offset: usize) -> Option<Comment> {
        if self.peek().is_some_and(|c| c.span.0 < offset) {
            let c = self.comments[self.idx].clone();
            self.idx += 1;
            Some(c)
        } else {
            None
        }
    }

    /// Next comment if it begins on `line` and at/after `min_offset`, consuming it.
    /// Used to fold a trailing comment onto the line of the code it follows.
    pub fn take_trailing(&mut self, line: usize, min_offset: usize) -> Option<Comment> {
        let hit = self
            .peek()
            .is_some_and(|c| c.span.0 >= min_offset && self.line_of(c.span.0) == line);
        if hit {
            let c = self.comments[self.idx].clone();
            self.idx += 1;
            Some(c)
        } else {
            None
        }
    }
}

/// The exact source text of a comment, trimmed of trailing whitespace. Line comments
/// keep their `//`; block comments keep their `/* */` and any internal newlines.
pub fn comment_text(c: &Comment) -> String {
    match c.kind {
        CommentKind::Line => c.text.trim_end().to_string(),
        CommentKind::Block => c.text.clone(),
    }
}
