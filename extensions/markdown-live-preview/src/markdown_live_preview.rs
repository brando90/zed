mod markdown;

use markdown::Style;
use zed_extension_api::{self as zed, Concealment, Decorations, Range, StyledRange, TextStyle};

struct MarkdownLivePreview;

impl zed::Extension for MarkdownLivePreview {
    fn new() -> Self {
        Self
    }

    fn decorate_buffer(&self, _decorator_id: &str, text: &str) -> Result<Decorations, String> {
        let decorations = markdown::decorate(text);
        let range = |range: std::ops::Range<usize>| Range {
            start: range.start as u32,
            end: range.end as u32,
        };

        Ok(Decorations {
            styles: decorations
                .styles
                .into_iter()
                .map(|(span, style)| StyledRange {
                    range: range(span),
                    style: text_style(style),
                })
                .collect(),
            concealments: decorations
                .concealments
                .into_iter()
                .map(|conceal| Concealment {
                    range: range(conceal.range),
                    replacement: conceal.replacement.map(str::to_string),
                    reveal: range(conceal.reveal),
                })
                .collect(),
        })
    }
}

fn text_style(style: Style) -> TextStyle {
    match style {
        Style::Bold => TextStyle::Bold,
        Style::Italic => TextStyle::Italic,
        Style::Strikethrough => TextStyle::Strikethrough,
        Style::Code => TextStyle::Code,
        Style::Link => TextStyle::Link,
        Style::Quote => TextStyle::Quote,
        Style::Dimmed => TextStyle::Dimmed,
        Style::Heading(1) => TextStyle::Heading1,
        Style::Heading(2) => TextStyle::Heading2,
        Style::Heading(3) => TextStyle::Heading3,
        Style::Heading(4) => TextStyle::Heading4,
        Style::Heading(5) => TextStyle::Heading5,
        Style::Heading(_) => TextStyle::Heading6,
    }
}

zed::register_extension!(MarkdownLivePreview);
