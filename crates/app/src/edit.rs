//! Pure text-offset helpers shared by the block editor and plain inputs.

use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

pub fn prev_grapheme(text: &str, offset: usize) -> usize {
    text.grapheme_indices(true)
        .rev()
        .find_map(|(i, _)| (i < offset).then_some(i))
        .unwrap_or(0)
}

pub fn next_grapheme(text: &str, offset: usize) -> usize {
    text.grapheme_indices(true)
        .find_map(|(i, _)| (i > offset).then_some(i))
        .unwrap_or(text.len())
}

fn is_word(s: &str) -> bool {
    s.chars().any(|c| c.is_alphanumeric() || c == '_')
}

/// Start of the word before `offset` (skipping whitespace and punctuation).
pub fn prev_word(text: &str, offset: usize) -> usize {
    let mut result = 0;
    let mut seen_word = false;
    for (i, w) in text.split_word_bound_indices().rev() {
        if i >= offset {
            continue;
        }
        if is_word(w) {
            result = i;
            seen_word = true;
            break;
        } else if seen_word {
            break;
        }
    }
    if seen_word {
        result
    } else {
        0
    }
}

/// End of the word after `offset`.
pub fn next_word(text: &str, offset: usize) -> usize {
    for (i, w) in text.split_word_bound_indices() {
        let end = i + w.len();
        if end <= offset {
            continue;
        }
        if is_word(w) {
            return end;
        }
    }
    text.len()
}

/// The word (or run of non-word characters) around `offset`.
pub fn word_range_at(text: &str, offset: usize) -> Range<usize> {
    for (i, w) in text.split_word_bound_indices() {
        let end = i + w.len();
        if offset >= i && offset < end {
            return i..end;
        }
    }
    let p = prev_word(text, offset);
    p..text.len().max(p)
}

pub fn offset_from_utf16(text: &str, offset: usize) -> usize {
    let mut utf8 = 0;
    let mut utf16 = 0;
    for ch in text.chars() {
        if utf16 >= offset {
            break;
        }
        utf16 += ch.len_utf16();
        utf8 += ch.len_utf8();
    }
    utf8
}

pub fn offset_to_utf16(text: &str, offset: usize) -> usize {
    let mut utf16 = 0;
    let mut utf8 = 0;
    for ch in text.chars() {
        if utf8 >= offset {
            break;
        }
        utf8 += ch.len_utf8();
        utf16 += ch.len_utf16();
    }
    utf16
}

pub fn range_to_utf16(text: &str, range: &Range<usize>) -> Range<usize> {
    offset_to_utf16(text, range.start)..offset_to_utf16(text, range.end)
}

pub fn range_from_utf16(text: &str, range: &Range<usize>) -> Range<usize> {
    offset_from_utf16(text, range.start)..offset_from_utf16(text, range.end)
}

/// Clamp an offset into the text and onto a char boundary.
pub fn clamp(text: &str, mut offset: usize) -> usize {
    offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

pub fn looks_like_url(s: &str) -> bool {
    let s = s.trim();
    (s.starts_with("http://") || s.starts_with("https://"))
        && !s.contains(char::is_whitespace)
        && s.len() > 10
}
