//! Finds what to hide and what to style in a Markdown text.

use std::ops::Range;

use pulldown_cmark::{Event, HeadingLevel, LinkType, Options, Parser, Tag, TagEnd};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Bold,
    Italic,
    Strikethrough,
    Code,
    Link,
    Quote,
    Dimmed,
    Heading(u8),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conceal {
    /// The text to hide or replace.
    pub range: Range<usize>,
    pub replacement: Option<&'static str>,
    /// The span whose touch by the cursor shows the source again.
    pub reveal: Range<usize>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Decorations {
    pub styles: Vec<(Range<usize>, Style)>,
    pub concealments: Vec<Conceal>,
}

/// An inline span with markers on both sides of its content, such as `**bold**`.
struct Open {
    kind: Marked,
    range: Range<usize>,
}

#[derive(Clone, Copy)]
enum Marked {
    Strong,
    Emphasis,
    Strikethrough,
    Link,
    /// A span whose end event must pop the stack, but that is not decorated,
    /// such as an autolink.
    Plain,
}

pub fn decorate(text: &str) -> Decorations {
    let mut out = Decorations::default();
    let bytes = text.as_bytes();
    let options =
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_TABLES;

    let mut open: Vec<Open> = Vec::new();
    let mut list_depth_markers: Vec<()> = Vec::new();
    let mut in_code_block = false;
    let mut quote_depth = 0usize;
    // The end of the last piece of content seen, used to find where the
    // closing marker of a link starts.
    let mut last_content_end = 0usize;

    for (event, range) in Parser::new_ext(text, options).into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                let hashes = bytes[range.start..]
                    .iter()
                    .take_while(|byte| **byte == b'#')
                    .count();
                let line_end = line_end(text, range.start);
                if hashes > 0 && hashes <= 6 {
                    let after = skip_spaces(bytes, range.start + hashes, line_end);
                    let content_end = trim_closing_hashes(text, after, line_end);
                    if after > range.start + hashes {
                        out.concealments.push(Conceal {
                            range: range.start..after,
                            replacement: None,
                            reveal: range.start..line_end,
                        });
                    }
                    if content_end < line_end {
                        out.concealments.push(Conceal {
                            range: content_end..line_end,
                            replacement: None,
                            reveal: range.start..line_end,
                        });
                    }
                    if after < content_end {
                        out.styles
                            .push((after..content_end, Style::Heading(heading_number(level))));
                    }
                } else {
                    // Setext heading: the text stays as it is, only styled.
                    out.styles
                        .push((range.clone(), Style::Heading(heading_number(level))));
                }
            }
            Event::Start(Tag::Strong) => open.push(Open {
                kind: Marked::Strong,
                range,
            }),
            Event::Start(Tag::Emphasis) => open.push(Open {
                kind: Marked::Emphasis,
                range,
            }),
            Event::Start(Tag::Strikethrough) => open.push(Open {
                kind: Marked::Strikethrough,
                range,
            }),
            Event::Start(Tag::Link { link_type, .. }) => open.push(Open {
                kind: match link_type {
                    LinkType::Inline
                    | LinkType::Reference
                    | LinkType::Shortcut
                    | LinkType::Collapsed => Marked::Link,
                    _ => Marked::Plain,
                },
                range,
            }),
            Event::End(
                TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough | TagEnd::Link,
            ) => {
                if let Some(Open { kind, range: span }) = open.pop() {
                    close_span(text, kind, span, last_content_end, &mut out);
                }
                // The closing marker is content too: a link around `**bold**`
                // finds its `]` after it.
                last_content_end = range.end;
            }
            Event::Code(_) => {
                let ticks = bytes[range.start..]
                    .iter()
                    .take_while(|byte| **byte == b'`')
                    .count();
                if ticks > 0 && range.len() >= ticks * 2 {
                    let content = range.start + ticks..range.end - ticks;
                    out.concealments.push(Conceal {
                        range: range.start..content.start,
                        replacement: None,
                        reveal: range.clone(),
                    });
                    out.concealments.push(Conceal {
                        range: content.end..range.end,
                        replacement: None,
                        reveal: range.clone(),
                    });
                    out.styles.push((content, Style::Code));
                }
                last_content_end = range.end;
            }
            Event::Start(Tag::Item) => {
                list_depth_markers.push(());
                if let Some(marker) = bullet_marker(bytes, range.start) {
                    let line_end = line_end(text, range.start);
                    out.concealments.push(Conceal {
                        range: marker..marker + 1,
                        replacement: Some("•"),
                        reveal: marker..line_end,
                    });
                }
            }
            Event::End(TagEnd::Item) => {
                list_depth_markers.pop();
            }
            Event::TaskListMarker(checked) => {
                // `[ ]` or `[x]`, three bytes.
                if range.len() == 3 {
                    out.concealments.push(Conceal {
                        range: range.clone(),
                        replacement: Some(if checked { "☑" } else { "☐" }),
                        reveal: range.clone(),
                    });
                }
            }
            Event::Start(Tag::BlockQuote(_)) => {
                quote_depth += 1;
                // A nested quote starts at its own marker: scan from the line start.
                let mut line_start = text[..range.start]
                    .rfind('\n')
                    .map_or(0, |newline| newline + 1);
                while line_start < range.end {
                    let end = line_end(text, line_start).min(range.end);
                    // The marker of this quote is the `quote_depth`-th `>` of the line.
                    let mut at = line_start;
                    let mut marker = None;
                    for _ in 0..quote_depth {
                        let quote_at = skip_spaces(bytes, at, end);
                        if bytes.get(quote_at) != Some(&b'>') || quote_at >= end {
                            marker = None;
                            break;
                        }
                        let after = if bytes.get(quote_at + 1) == Some(&b' ') && quote_at + 1 < end
                        {
                            quote_at + 2
                        } else {
                            quote_at + 1
                        };
                        marker = Some(quote_at..after);
                        at = after;
                    }
                    if let Some(marker) = marker {
                        out.concealments.push(Conceal {
                            range: marker,
                            replacement: Some("┃ "),
                            reveal: line_start..end,
                        });
                    }
                    line_start = next_line_start(bytes, end);
                }
                out.styles.push((range, Style::Quote));
            }
            Event::End(TagEnd::BlockQuote(_)) => quote_depth -= 1,
            Event::Start(Tag::CodeBlock(_)) => {
                in_code_block = true;
                out.styles.push((range, Style::Code));
            }
            Event::End(TagEnd::CodeBlock) => in_code_block = false,
            Event::Rule => out.styles.push((range, Style::Dimmed)),
            Event::End(TagEnd::Image) | Event::InlineHtml(_) => last_content_end = range.end,
            Event::Text(_) | Event::SoftBreak | Event::HardBreak => {
                if !in_code_block {
                    last_content_end = range.end;
                }
            }
            _ => {}
        }
    }
    out
}

fn close_span(
    text: &str,
    kind: Marked,
    span: Range<usize>,
    last_content_end: usize,
    out: &mut Decorations,
) {
    let bytes = text.as_bytes();
    match kind {
        Marked::Link => {
            // `[text](url)`: hide `[` and everything from the closing `]` on.
            if bytes.get(span.start) == Some(&b'[') && last_content_end > span.start {
                let content = span.start + 1..last_content_end;
                if bytes.get(content.end) == Some(&b']') && content.start <= content.end {
                    out.concealments.push(Conceal {
                        range: span.start..content.start,
                        replacement: None,
                        reveal: span.clone(),
                    });
                    out.concealments.push(Conceal {
                        range: content.end..span.end,
                        replacement: None,
                        reveal: span.clone(),
                    });
                    if content.start < content.end {
                        out.styles.push((content, Style::Link));
                    }
                }
            }
        }
        Marked::Plain => {}
        Marked::Strong | Marked::Emphasis | Marked::Strikethrough => {
            let marker_len = match kind {
                Marked::Emphasis => 1,
                // `~x~` and `~~x~~` both strike through.
                Marked::Strikethrough => {
                    let run = |text: &[u8]| text.iter().take_while(|byte| **byte == b'~').count();
                    let start = run(&bytes[span.start..]);
                    let end = bytes[span.start..span.end]
                        .iter()
                        .rev()
                        .take_while(|byte| **byte == b'~')
                        .count();
                    start.min(end).clamp(1, 2)
                }
                _ => 2,
            };
            let marker_char = bytes.get(span.start).copied();
            let is_marker = |at: usize| {
                bytes
                    .get(at..at + marker_len)
                    .is_some_and(|marker| marker.iter().all(|byte| Some(*byte) == marker_char))
            };
            if span.len() > marker_len * 2
                && matches!(marker_char, Some(b'*' | b'_' | b'~'))
                && is_marker(span.start)
                && is_marker(span.end - marker_len)
            {
                let content = span.start + marker_len..span.end - marker_len;
                out.concealments.push(Conceal {
                    range: span.start..content.start,
                    replacement: None,
                    reveal: span.clone(),
                });
                out.concealments.push(Conceal {
                    range: content.end..span.end,
                    replacement: None,
                    reveal: span.clone(),
                });
                out.styles.push((
                    content,
                    match kind {
                        Marked::Strong => Style::Bold,
                        Marked::Emphasis => Style::Italic,
                        _ => Style::Strikethrough,
                    },
                ));
            }
        }
    }
}

fn heading_number(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// The end of the line that contains `at`, before its newline.
fn line_end(text: &str, at: usize) -> usize {
    let end = text[at..]
        .find('\n')
        .map_or(text.len(), |newline| at + newline);
    if end > at && text.as_bytes()[end - 1] == b'\r' {
        end - 1
    } else {
        end
    }
}

/// Where the line after the one that ends at `end` (from [`line_end`]) starts.
fn next_line_start(bytes: &[u8], mut end: usize) -> usize {
    if bytes.get(end) == Some(&b'\r') {
        end += 1;
    }
    if bytes.get(end) == Some(&b'\n') {
        end += 1;
    }
    end
}

fn skip_spaces(bytes: &[u8], mut at: usize, end: usize) -> usize {
    while at < end && matches!(bytes[at], b' ' | b'\t') {
        at += 1;
    }
    at
}

/// Where the content of an ATX heading ends: before its optional closing `###`.
fn trim_closing_hashes(text: &str, start: usize, end: usize) -> usize {
    let line = text[start..end].trim_end_matches([' ', '\t']);
    let without_hashes = line.trim_end_matches('#');
    if without_hashes.len() == line.len() {
        return start + line.len();
    }
    if without_hashes.is_empty() || without_hashes.ends_with([' ', '\t']) {
        start + without_hashes.trim_end_matches([' ', '\t']).len()
    } else {
        start + line.len()
    }
}

/// The offset of the bullet (`-`, `*` or `+`) that starts a list item, if the
/// item has one rather than a number.
fn bullet_marker(bytes: &[u8], item_start: usize) -> Option<usize> {
    let at = item_start;
    match bytes.get(at) {
        Some(b'-' | b'*' | b'+')
            if matches!(bytes.get(at + 1), Some(b' ' | b'\t' | b'\r' | b'\n') | None) =>
        {
            Some(at)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn concealed<'a>(text: &'a str, decorations: &Decorations) -> Vec<&'a str> {
        decorations
            .concealments
            .iter()
            .map(|conceal| &text[conceal.range.clone()])
            .collect()
    }

    fn styled<'a>(text: &'a str, decorations: &Decorations, style: Style) -> Vec<&'a str> {
        decorations
            .styles
            .iter()
            .filter(|(_, candidate)| *candidate == style)
            .map(|(range, _)| &text[range.clone()])
            .collect()
    }

    #[test]
    fn bold_hides_markers_and_styles_content() {
        let text = "a **bold** b";
        let decorations = decorate(text);
        assert_eq!(concealed(text, &decorations), vec!["**", "**"]);
        assert_eq!(styled(text, &decorations, Style::Bold), vec!["bold"]);
        // Touching any part of the token reveals both markers.
        assert!(
            decorations
                .concealments
                .iter()
                .all(|conceal| &text[conceal.reveal.clone()] == "**bold**")
        );
    }

    #[test]
    fn italic_and_strikethrough() {
        let text = "*it* and ~~gone~~ and _it2_";
        let decorations = decorate(text);
        assert_eq!(
            concealed(text, &decorations),
            vec!["*", "*", "~~", "~~", "_", "_"]
        );
        assert_eq!(styled(text, &decorations, Style::Italic), vec!["it", "it2"]);
        assert_eq!(
            styled(text, &decorations, Style::Strikethrough),
            vec!["gone"]
        );
    }

    #[test]
    fn bold_italic_nests() {
        let text = "***both***";
        let decorations = decorate(text);
        // Both marker pairs are hidden and the content is styled twice.
        assert_eq!(decorations.concealments.len(), 4);
        assert_eq!(styled(text, &decorations, Style::Bold), vec!["both"]);
        assert_eq!(styled(text, &decorations, Style::Italic), vec!["**both**"]);
    }

    #[test]
    fn headings_hide_hashes() {
        let text = "# Title\n\n## Sub ##\n";
        let decorations = decorate(text);
        assert_eq!(concealed(text, &decorations), vec!["# ", "## ", " ##"]);
        assert_eq!(styled(text, &decorations, Style::Heading(1)), vec!["Title"]);
        assert_eq!(styled(text, &decorations, Style::Heading(2)), vec!["Sub"]);
    }

    #[test]
    fn inline_code_hides_backticks() {
        let text = "use `foo()` here and ``a ` b``";
        let decorations = decorate(text);
        assert_eq!(concealed(text, &decorations), vec!["`", "`", "``", "``"]);
        assert_eq!(
            styled(text, &decorations, Style::Code),
            vec!["foo()", "a ` b"]
        );
    }

    #[test]
    fn links_hide_brackets_and_target() {
        let text = "see [the docs](https://zed.dev/docs) now";
        let decorations = decorate(text);
        assert_eq!(
            concealed(text, &decorations),
            vec!["[", "](https://zed.dev/docs)"]
        );
        assert_eq!(styled(text, &decorations, Style::Link), vec!["the docs"]);
    }

    #[test]
    fn bullets_and_tasks_are_replaced() {
        let text = "- one\n* two\n- [ ] todo\n- [x] done\n1. numbered\n";
        let decorations = decorate(text);
        let replaced: Vec<_> = decorations
            .concealments
            .iter()
            .filter_map(|conceal| conceal.replacement)
            .collect();
        assert_eq!(replaced, vec!["•", "•", "•", "☐", "•", "☑"]);
        // A numbered item keeps its number.
        assert!(!concealed(text, &decorations).contains(&"1"));
    }

    #[test]
    fn block_quote_marker_is_replaced_on_each_line() {
        let text = "> one\n> two\n";
        let decorations = decorate(text);
        assert_eq!(concealed(text, &decorations), vec!["> ", "> "]);
        assert!(
            decorations
                .concealments
                .iter()
                .all(|conceal| conceal.replacement == Some("┃ "))
        );
    }

    #[test]
    fn code_blocks_are_left_alone() {
        let text = "```rust\nlet a = **not bold**;\n```\n";
        let decorations = decorate(text);
        assert!(decorations.concealments.is_empty());
    }

    #[test]
    fn unbalanced_markers_are_not_hidden() {
        let text = "a * b ** c _ d";
        assert!(decorate(text).concealments.is_empty());
    }

    #[test]
    fn multibyte_text_never_splits_a_character() {
        let text = "# héllo **wörld** `ç` [ü](x)\n- ñ\n";
        let decorations = decorate(text);
        for conceal in &decorations.concealments {
            assert!(text.is_char_boundary(conceal.range.start));
            assert!(text.is_char_boundary(conceal.range.end));
        }
        for (range, _) in &decorations.styles {
            assert!(text.is_char_boundary(range.start));
            assert!(text.is_char_boundary(range.end));
        }
    }

    #[test]
    fn an_autolink_inside_emphasis_does_not_end_the_emphasis() {
        let text = "*see <https://zed.dev> now*";
        let decorations = decorate(text);
        assert_eq!(concealed(text, &decorations), vec!["*", "*"]);
        assert_eq!(
            styled(text, &decorations, Style::Italic),
            vec!["see <https://zed.dev> now"]
        );
    }

    #[test]
    fn a_link_around_bold_text_is_still_a_link() {
        let text = "[**bold**](https://zed.dev)";
        let decorations = decorate(text);
        assert_eq!(
            concealed(text, &decorations),
            vec!["**", "**", "[", "](https://zed.dev)"]
        );
        assert_eq!(styled(text, &decorations, Style::Link), vec!["**bold**"]);
    }

    #[test]
    fn crlf_lines_keep_their_line_break() {
        let text = "## Title ##\r\n> quote\r\n- item\r\n";
        let decorations = decorate(text);
        assert_eq!(concealed(text, &decorations), vec!["## ", " ##", "> ", "-"]);
        assert_eq!(styled(text, &decorations, Style::Heading(2)), vec!["Title"]);
    }

    #[test]
    fn nested_quotes_hide_one_marker_per_level() {
        let text = "> outer\n> > inner\n";
        let decorations = decorate(text);
        // Each line has its own markers, one concealment per `>`.
        assert_eq!(concealed(text, &decorations), vec!["> ", "> ", "> "]);
        let starts: Vec<usize> = decorations
            .concealments
            .iter()
            .map(|conceal| conceal.range.start)
            .collect();
        let mut unique = starts.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), starts.len(), "no marker is hidden twice");
    }

    #[test]
    fn single_tilde_strikethrough_and_empty_bullets() {
        let text = "~gone~ and ~~also~~\n\n-\n";
        let decorations = decorate(text);
        assert_eq!(
            styled(text, &decorations, Style::Strikethrough),
            vec!["gone", "also"]
        );
        assert!(concealed(text, &decorations).contains(&"-"));
    }
}
