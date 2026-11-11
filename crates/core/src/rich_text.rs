//! Rich text: a string plus run-length encoded inline marks.
//!
//! `runs` always covers the whole text (`sum(run.len) == text.len()`), offsets
//! are UTF-8 byte offsets and must lie on char boundaries.

use std::ops::Range;

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Marks {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub code: bool,
    pub link: Option<String>,
}

impl Marks {
    pub fn is_plain(&self) -> bool {
        *self == Marks::default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkKind {
    Bold,
    Italic,
    Underline,
    Strike,
    Code,
}

impl Marks {
    pub fn get(&self, kind: MarkKind) -> bool {
        match kind {
            MarkKind::Bold => self.bold,
            MarkKind::Italic => self.italic,
            MarkKind::Underline => self.underline,
            MarkKind::Strike => self.strike,
            MarkKind::Code => self.code,
        }
    }

    pub fn set(&mut self, kind: MarkKind, on: bool) {
        match kind {
            MarkKind::Bold => self.bold = on,
            MarkKind::Italic => self.italic = on,
            MarkKind::Underline => self.underline = on,
            MarkKind::Strike => self.strike = on,
            MarkKind::Code => self.code = on,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub len: usize,
    pub marks: Marks,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RichText {
    pub text: String,
    pub runs: Vec<Run>,
}

impl RichText {
    pub fn plain(text: impl Into<String>) -> Self {
        let text = text.into();
        let mut rt = RichText {
            runs: vec![Run {
                len: text.len(),
                marks: Marks::default(),
            }],
            text,
        };
        rt.normalize();
        rt
    }

    pub fn len(&self) -> usize {
        self.text.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn push(&mut self, text: &str, marks: Marks) {
        if text.is_empty() {
            return;
        }
        self.text.push_str(text);
        self.runs.push(Run {
            len: text.len(),
            marks,
        });
        self.normalize();
    }

    /// Marks that newly typed text at `offset` should inherit: the marks of the
    /// character just before the caret (links are not extended).
    pub fn marks_at(&self, offset: usize) -> Marks {
        let mut pos = 0;
        let mut found = Marks::default();
        for run in &self.runs {
            if run.len == 0 {
                continue;
            }
            let end = pos + run.len;
            if offset > pos && offset <= end {
                found = run.marks.clone();
                break;
            }
            if offset == 0 {
                found = run.marks.clone();
                break;
            }
            pos = end;
        }
        // Typing after a link or inline code at its very end should not extend it
        // forever; Notion keeps code but drops links. We do the same.
        if found.link.is_some() && self.is_run_end(offset) {
            found.link = None;
        }
        found
    }

    fn is_run_end(&self, offset: usize) -> bool {
        let mut pos = 0;
        for run in &self.runs {
            pos += run.len;
            if pos == offset {
                return true;
            }
        }
        false
    }

    /// Marks of the character starting at `offset` (if any).
    pub fn marks_of_char(&self, offset: usize) -> Option<&Marks> {
        let mut pos = 0;
        for run in &self.runs {
            let end = pos + run.len;
            if offset >= pos && offset < end {
                return Some(&run.marks);
            }
            pos = end;
        }
        None
    }

    /// Split runs so that `offset` is a run boundary; returns the index of the
    /// first run starting at `offset`.
    fn split_at(&mut self, offset: usize) -> usize {
        let mut pos = 0;
        for i in 0..self.runs.len() {
            let end = pos + self.runs[i].len;
            if offset == pos {
                return i;
            }
            if offset < end {
                let left = offset - pos;
                let right = end - offset;
                let marks = self.runs[i].marks.clone();
                self.runs[i].len = left;
                self.runs.insert(i + 1, Run { len: right, marks });
                return i + 1;
            }
            pos = end;
        }
        self.runs.len()
    }

    /// Replace `range` with `new_text`. Inserted text uses `marks`, or inherits
    /// from the surrounding text when `None`.
    pub fn replace(&mut self, range: Range<usize>, new_text: &str, marks: Option<Marks>) {
        let range = range.start.min(self.text.len())..range.end.min(self.text.len());
        let marks = marks.unwrap_or_else(|| {
            if range.is_empty() {
                self.marks_at(range.start)
            } else {
                // Replacing a selection: take the marks of its first character.
                let mut m = self.marks_of_char(range.start).cloned().unwrap_or_default();
                if new_text.is_empty() {
                    m = Marks::default();
                }
                m
            }
        });
        let start_ix = self.split_at(range.start);
        let end_ix = self.split_at(range.end);
        self.runs.drain(start_ix..end_ix);
        if !new_text.is_empty() {
            self.runs.insert(
                start_ix,
                Run {
                    len: new_text.len(),
                    marks,
                },
            );
        }
        self.text.replace_range(range, new_text);
        self.normalize();
    }

    pub fn slice(&self, range: Range<usize>) -> RichText {
        let mut out = RichText::default();
        let mut pos = 0;
        for run in &self.runs {
            let end = pos + run.len;
            let s = range.start.max(pos);
            let e = range.end.min(end);
            if s < e {
                out.text.push_str(&self.text[s..e]);
                out.runs.push(Run {
                    len: e - s,
                    marks: run.marks.clone(),
                });
            }
            pos = end;
        }
        out.normalize();
        out
    }

    pub fn split_off(&mut self, at: usize) -> RichText {
        let tail = self.slice(at..self.text.len());
        *self = self.slice(0..at);
        tail
    }

    pub fn append(&mut self, other: &RichText) {
        self.text.push_str(&other.text);
        self.runs.extend(other.runs.iter().cloned());
        self.normalize();
    }

    /// Insert rich text at `at`.
    pub fn insert_rich(&mut self, at: usize, other: &RichText) {
        let tail = self.split_off(at);
        self.append(other);
        self.append(&tail);
    }

    pub fn range_has(&self, range: Range<usize>, kind: MarkKind) -> bool {
        if range.is_empty() {
            return false;
        }
        let mut pos = 0;
        for run in &self.runs {
            let end = pos + run.len;
            if run.len > 0 && pos < range.end && end > range.start && !run.marks.get(kind) {
                return false;
            }
            pos = end;
        }
        true
    }

    fn update_range(&mut self, range: Range<usize>, f: impl Fn(&mut Marks)) {
        if range.is_empty() {
            return;
        }
        let start_ix = self.split_at(range.start);
        let end_ix = self.split_at(range.end);
        for run in &mut self.runs[start_ix..end_ix] {
            f(&mut run.marks);
        }
        self.normalize();
    }

    pub fn set_mark(&mut self, range: Range<usize>, kind: MarkKind, on: bool) {
        self.update_range(range, |m| m.set(kind, on));
    }

    /// Toggle a mark over a range: removes it if the whole range has it.
    pub fn toggle(&mut self, range: Range<usize>, kind: MarkKind) {
        let on = !self.range_has(range.clone(), kind);
        self.set_mark(range, kind, on);
    }

    pub fn set_link(&mut self, range: Range<usize>, link: Option<String>) {
        self.update_range(range, |m| m.link = link.clone());
    }

    pub fn clear_marks(&mut self) {
        self.runs = vec![Run {
            len: self.text.len(),
            marks: Marks::default(),
        }];
        self.normalize();
    }

    pub fn normalize(&mut self) {
        let mut out: Vec<Run> = Vec::with_capacity(self.runs.len());
        for run in self.runs.drain(..) {
            if run.len == 0 {
                continue;
            }
            match out.last_mut() {
                Some(last) if last.marks == run.marks => last.len += run.len,
                _ => out.push(run),
            }
        }
        let total: usize = out.iter().map(|r| r.len).sum();
        if total < self.text.len() {
            out.push(Run {
                len: self.text.len() - total,
                marks: Marks::default(),
            });
        } else if total > self.text.len() {
            // Should never happen; repair defensively.
            out = vec![Run {
                len: self.text.len(),
                marks: Marks::default(),
            }];
            out.retain(|r| r.len > 0);
        }
        self.runs = out;
    }

    /// Iterate `(byte_range, marks)`.
    pub fn segments(&self) -> impl Iterator<Item = (Range<usize>, &Marks)> {
        let mut pos = 0;
        self.runs.iter().map(move |run| {
            let r = pos..pos + run.len;
            pos += run.len;
            (r, &run.marks)
        })
    }
}

impl From<&str> for RichText {
    fn from(s: &str) -> Self {
        RichText::plain(s)
    }
}

impl From<String> for RichText {
    fn from(s: String) -> Self {
        RichText::plain(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_and_inherit() {
        let mut t = RichText::plain("hello world");
        t.toggle(0..5, MarkKind::Bold);
        assert_eq!(t.runs.len(), 2);
        t.replace(5..5, "!!", None);
        assert_eq!(t.text, "hello!! world");
        assert!(t.range_has(0..7, MarkKind::Bold));
        assert!(!t.range_has(0..8, MarkKind::Bold));
        t.replace(0..7, "", None);
        assert_eq!(t.text, " world");
        assert_eq!(t.runs.len(), 1);
    }

    #[test]
    fn split_and_append() {
        let mut t = RichText::plain("abcdef");
        t.toggle(2..4, MarkKind::Italic);
        let tail = t.split_off(3);
        assert_eq!(t.text, "abc");
        assert_eq!(tail.text, "def");
        assert!(tail.range_has(0..1, MarkKind::Italic));
        t.append(&tail);
        assert_eq!(t.text, "abcdef");
        assert!(t.range_has(2..4, MarkKind::Italic));
        assert_eq!(t.runs.len(), 3);
    }

    #[test]
    fn toggle_off() {
        let mut t = RichText::plain("abc");
        t.toggle(0..3, MarkKind::Code);
        t.toggle(1..2, MarkKind::Code);
        assert_eq!(t.runs.len(), 3);
        t.toggle(0..3, MarkKind::Code);
        assert!(t.range_has(0..3, MarkKind::Code));
    }
}
