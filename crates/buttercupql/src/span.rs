//! Places in a ButtercupQL source text.

use std::ops::Range;

/// A range of the source text, in bytes: `start` is the first byte and `end` the byte after the last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Span {
    /// The first byte.
    pub start: u32,
    /// The byte after the last one.
    pub end: u32,
}

impl Span {
    /// The span from byte `start` to byte `end`.
    pub fn new(start: usize, end: usize) -> Span {
        debug_assert!(start <= end, "a span ends before it starts");
        Span {
            start: start as u32,
            end: end as u32,
        }
    }

    /// An empty span at byte `at`, the place of something missing.
    pub fn empty(at: usize) -> Span {
        Span::new(at, at)
    }

    /// The span that covers both this one and `other`.
    pub fn to(self, other: Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    /// The bytes the span covers, to slice the source with.
    pub fn range(self) -> Range<usize> {
        self.start as usize..self.end as usize
    }

    /// Whether the span covers no text.
    pub fn is_empty(self) -> bool {
        self.start == self.end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_span_covers_both_ends_and_slices_its_text() {
        let text = "from cases";
        let from = Span::new(0, 4);
        let cases = Span::new(5, 10);
        assert_eq!(&text[from.range()], "from");
        assert_eq!(&text[from.to(cases).range()], "from cases");
        assert_eq!(cases.to(from), from.to(cases));
        assert!(Span::empty(4).is_empty());
    }
}
