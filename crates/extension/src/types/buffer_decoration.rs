use std::ops::Range;

/// A theme-driven text style that a decorator can ask for.
///
/// Extensions pick a semantic style, never a color or a font: the host maps
/// each style to the active theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextStyle {
    Bold,
    Italic,
    BoldItalic,
    Strikethrough,
    Code,
    Link,
    Quote,
    Dimmed,
    Heading1,
    Heading2,
    Heading3,
    Heading4,
    Heading5,
    Heading6,
}

/// A byte range of the decorated text that is drawn with a style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyledRange {
    pub range: Range<usize>,
    pub style: TextStyle,
}

/// A byte range of the decorated text that is hidden or drawn as a short
/// replacement, until a cursor or selection touches `reveal`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Concealment {
    pub range: Range<usize>,
    pub replacement: Option<String>,
    pub reveal: Range<usize>,
}

/// What a decorator asks the editor to draw over a buffer. All ranges are
/// byte offsets into the text that was passed to the decorator.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BufferDecorations {
    pub styles: Vec<StyledRange>,
    pub concealments: Vec<Concealment>,
}

/// Limits that the host enforces on what a decorator returns.
pub const MAX_DECORATED_TEXT_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_DECORATIONS: usize = 50_000;
pub const MAX_REPLACEMENT_BYTES: usize = 16;

impl BufferDecorations {
    /// Drops everything that is malformed for a text of `text` and returns the
    /// rest: ranges that are empty, out of bounds, not on character
    /// boundaries, or concealments that overlap an earlier one; replacements
    /// that are too long; and anything past [`MAX_DECORATIONS`]. A decorator
    /// can therefore never make the editor panic or hide the same text twice.
    pub fn sanitized(mut self, text: &str) -> Self {
        let valid = |range: &Range<usize>| {
            range.start < range.end
                && range.end <= text.len()
                && text.is_char_boundary(range.start)
                && text.is_char_boundary(range.end)
        };
        self.styles.retain(|style| valid(&style.range));
        self.styles.truncate(MAX_DECORATIONS);

        self.concealments.retain(|concealment| {
            valid(&concealment.range)
                && valid(&concealment.reveal)
                && concealment
                    .replacement
                    .as_ref()
                    .is_none_or(|text| text.len() <= MAX_REPLACEMENT_BYTES)
        });
        self.concealments
            .sort_by_key(|concealment| concealment.range.start);
        let mut end_of_previous = 0;
        self.concealments.retain(|concealment| {
            let keep = concealment.range.start >= end_of_previous;
            if keep {
                end_of_previous = concealment.range.end;
            }
            keep
        });
        self.concealments.truncate(MAX_DECORATIONS);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn concealment(range: Range<usize>) -> Concealment {
        Concealment {
            reveal: range.clone(),
            range,
            replacement: None,
        }
    }

    #[test]
    fn sanitizing_drops_malformed_ranges() {
        let text = "a**é**b";
        let decorations = BufferDecorations {
            styles: vec![
                StyledRange {
                    range: 3..5,
                    style: TextStyle::Bold,
                },
                StyledRange {
                    range: 4..4,
                    style: TextStyle::Bold,
                },
                StyledRange {
                    range: 0..99,
                    style: TextStyle::Bold,
                },
                StyledRange {
                    range: 3..5,
                    style: TextStyle::Italic,
                },
            ],
            concealments: vec![
                concealment(1..3),
                concealment(2..4),
                concealment(5..7),
                concealment(6..99),
            ],
        }
        .sanitized(text);

        // 3..5 cuts "é" (bytes 3..5 are one char, so 3..5 is valid); 4..4 is empty and
        // 0..99 is out of bounds.
        assert_eq!(decorations.styles.len(), 2);
        // 2..4 overlaps 1..3 and is dropped; 6..99 is out of bounds.
        let ranges: Vec<_> = decorations
            .concealments
            .iter()
            .map(|concealment| concealment.range.clone())
            .collect();
        assert_eq!(ranges, vec![1..3, 5..7]);
    }

    #[test]
    fn sanitizing_rejects_long_replacements_and_mid_character_ranges() {
        let text = "é**";
        let decorations = BufferDecorations {
            styles: vec![StyledRange {
                range: 1..3,
                style: TextStyle::Bold,
            }],
            concealments: vec![Concealment {
                range: 2..4,
                replacement: Some("x".repeat(MAX_REPLACEMENT_BYTES + 1)),
                reveal: 2..4,
            }],
        }
        .sanitized(text);
        assert!(decorations.styles.is_empty());
        assert!(decorations.concealments.is_empty());
    }
}
