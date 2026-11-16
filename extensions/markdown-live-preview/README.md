# Markdown Live Preview

Edits Markdown in the editor, with the syntax markers of the text hidden until
the cursor touches them, like Obsidian's live preview. The buffer stays plain
text: nothing is rewritten, saved differently, or rendered in a separate pane.

It uses the buffer decorator extension point (`[buffer_decorators.*]` in
`extension.toml` and `Extension::decorate_buffer` in the Rust API), which asks an
extension for theme-driven text styles and for ranges to hide.

What it does today:

- Headings: hides `#` markers, draws the title in bold in the theme's title color.
- Bold, italic, bold italic and strikethrough: hides `**`, `*`, `~~`.
- Inline code: hides the backticks.
- Links: hides `[` and `](url)`; the text is drawn as a link.
- Bullets: `-`, `*` and `+` are drawn as `•`; task boxes as `☐` and `☑`.
- Block quotes: `>` is drawn as `┃`.

What it cannot do yet, because the decoration API has no way to ask for it:
different heading sizes, rendered tables, images and diagrams (they need inline
blocks, a second step of the API).

Try it on a development build of Zed: `zed: install dev extension`, then pick
this folder.
