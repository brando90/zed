use extension::TextStyle;
use gpui::{App, FontStyle, FontWeight, HighlightStyle, StrikethroughStyle, px};
use theme::ActiveTheme as _;

/// Every style, in a fixed order that gives each one its own highlight key.
pub(crate) const ALL_STYLES: [TextStyle; 14] = [
    TextStyle::Bold,
    TextStyle::Italic,
    TextStyle::BoldItalic,
    TextStyle::Strikethrough,
    TextStyle::Code,
    TextStyle::Link,
    TextStyle::Quote,
    TextStyle::Dimmed,
    TextStyle::Heading1,
    TextStyle::Heading2,
    TextStyle::Heading3,
    TextStyle::Heading4,
    TextStyle::Heading5,
    TextStyle::Heading6,
];

/// Maps a semantic style to the active theme: the colors come from the syntax
/// theme (so light and dark themes both work), and the weight and slant are
/// set here so that they hold even when a theme leaves a token unstyled.
pub fn highlight_style(style: TextStyle, cx: &App) -> HighlightStyle {
    let syntax = cx.theme().syntax();
    let themed = |name: &str| syntax.style_for_name(name).unwrap_or_default();

    match style {
        TextStyle::Bold => HighlightStyle {
            font_weight: Some(FontWeight::BOLD),
            ..themed("emphasis.strong")
        },
        TextStyle::Italic => HighlightStyle {
            font_style: Some(FontStyle::Italic),
            ..themed("emphasis")
        },
        TextStyle::BoldItalic => HighlightStyle {
            font_weight: Some(FontWeight::BOLD),
            font_style: Some(FontStyle::Italic),
            ..themed("emphasis.strong")
        },
        TextStyle::Strikethrough => HighlightStyle {
            strikethrough: Some(StrikethroughStyle {
                thickness: px(1.),
                color: None,
            }),
            ..Default::default()
        },
        TextStyle::Code => themed("text.literal"),
        TextStyle::Link => themed("link_text"),
        TextStyle::Quote => HighlightStyle {
            font_style: Some(FontStyle::Italic),
            ..themed("comment")
        },
        TextStyle::Dimmed => HighlightStyle {
            fade_out: Some(0.5),
            ..Default::default()
        },
        // Headings share the theme's title color and are bold. Different sizes
        // need font-size support in text highlights, which does not exist yet.
        TextStyle::Heading1
        | TextStyle::Heading2
        | TextStyle::Heading3
        | TextStyle::Heading4
        | TextStyle::Heading5
        | TextStyle::Heading6 => HighlightStyle {
            font_weight: Some(FontWeight::BOLD),
            ..themed("title")
        },
    }
}
