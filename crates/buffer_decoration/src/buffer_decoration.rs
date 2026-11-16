//! Draws what extensions ask for over buffers.
//!
//! An extension declares a buffer decorator for some languages. While an editor
//! shows a buffer in one of those languages, this crate asks the decorator
//! (after a short delay, when the text changes) for theme-driven text styles and
//! for ranges to hide. It never lets a decorator change the buffer, and it
//! re-shows the source of a hidden range while a cursor or selection touches it,
//! so the extension is not called when the cursor moves.

mod style;

use std::{any::Any, ops::Range, sync::Arc, time::Duration};

use collections::BTreeMap;
use editor::{Addon, Concealment, Editor, EditorEvent, HighlightKey, MultiBufferOffset};
use extension::{
    BufferDecorations, Extension, ExtensionBufferDecoratorProxy, ExtensionHostProxy,
    MAX_DECORATED_TEXT_BYTES, TextStyle,
};
use gpui::{App, AppContext as _, Context, Entity, EventEmitter, SharedString, Task};
use multi_buffer::{Anchor, ToOffset as _};

/// How long the text of a buffer must stay unchanged before the decorator is called.
const DECORATE_DEBOUNCE: Duration = Duration::from_millis(150);

pub use style::highlight_style;

pub fn init(proxy: Arc<ExtensionHostProxy>, cx: &mut App) {
    let registry = cx.new(|_| DecoratorRegistry::default());

    proxy.register_buffer_decorator_proxy(RegistryProxy {
        registry: registry.clone(),
    });

    cx.observe_new(move |editor: &mut Editor, _, cx| {
        register_editor(editor, registry.clone(), cx);
    })
    .detach();
}

/// The buffer decorators that loaded extensions provide.
#[derive(Default)]
struct DecoratorRegistry {
    decorators: BTreeMap<Arc<str>, RegisteredDecorator>,
}

#[derive(Clone)]
struct RegisteredDecorator {
    id: Arc<str>,
    extension: Arc<dyn Extension>,
    languages: Vec<Arc<str>>,
}

struct DecoratorsChanged;

impl EventEmitter<DecoratorsChanged> for DecoratorRegistry {}

impl DecoratorRegistry {
    fn decorator_for_language(&self, language: &str) -> Option<RegisteredDecorator> {
        self.decorators
            .values()
            .find(|decorator| decorator.languages.iter().any(|name| &**name == language))
            .cloned()
    }
}

/// Decorator ids are only unique within an extension, so the registry is keyed
/// by extension id and decorator id.
fn decorator_key(extension_id: &str, decorator_id: &str) -> Arc<str> {
    format!("{extension_id}/{decorator_id}").into()
}

struct RegistryProxy {
    registry: Entity<DecoratorRegistry>,
}

impl ExtensionBufferDecoratorProxy for RegistryProxy {
    fn register_buffer_decorator(
        &self,
        extension: Arc<dyn Extension>,
        decorator_id: Arc<str>,
        languages: Vec<Arc<str>>,
        cx: &mut App,
    ) {
        let key = decorator_key(&extension.manifest().id, &decorator_id);
        self.registry.update(cx, |registry, cx| {
            registry.decorators.insert(
                key,
                RegisteredDecorator {
                    id: decorator_id,
                    extension,
                    languages,
                },
            );
            cx.emit(DecoratorsChanged);
        });
    }

    fn unregister_buffer_decorator(
        &self,
        extension_id: Arc<str>,
        decorator_id: Arc<str>,
        cx: &mut App,
    ) {
        let key = decorator_key(&extension_id, &decorator_id);
        self.registry.update(cx, |registry, cx| {
            registry.decorators.remove(&key);
            cx.emit(DecoratorsChanged);
        });
    }
}

/// A concealment of the decorated text, kept as anchors so it follows edits.
#[derive(Clone, Debug)]
struct AnchoredConcealment {
    range: Range<Anchor>,
    replacement: Option<SharedString>,
    reveal: Range<Anchor>,
}

/// What the decorator last returned for an editor, as anchors.
#[derive(Default)]
struct AppliedDecorations {
    styles: Vec<(TextStyle, Range<Anchor>)>,
    concealments: Vec<AnchoredConcealment>,
}

/// The per-editor state of buffer decoration.
struct BufferDecorationAddon {
    registry: Entity<DecoratorRegistry>,
    applied: AppliedDecorations,
    refresh: Task<()>,
}

impl Addon for BufferDecorationAddon {
    fn to_any(&self) -> &dyn Any {
        self
    }

    fn to_any_mut(&mut self) -> Option<&mut dyn Any> {
        Some(self)
    }
}

fn register_editor(
    editor: &mut Editor,
    registry: Entity<DecoratorRegistry>,
    cx: &mut Context<Editor>,
) {
    if !editor.mode().is_full() || !editor.buffer().read(cx).is_singleton() {
        return;
    }

    editor.register_addon(BufferDecorationAddon {
        registry: registry.clone(),
        applied: AppliedDecorations::default(),
        refresh: Task::ready(()),
    });

    cx.subscribe_self(|editor, event: &EditorEvent, cx| match event {
        EditorEvent::BufferEdited | EditorEvent::Reparsed(_) => schedule_refresh(editor, cx),
        EditorEvent::SelectionsChanged { .. } => apply(editor, cx),
        _ => {}
    })
    .detach();

    cx.subscribe(&registry, |editor, _, _: &DecoratorsChanged, cx| {
        schedule_refresh(editor, cx)
    })
    .detach();

    schedule_refresh(editor, cx);
}

/// Asks the decorator for the buffer's language, after a short delay, for new
/// decorations, and applies them.
fn schedule_refresh(editor: &mut Editor, cx: &mut Context<Editor>) {
    let Some(buffer) = editor.buffer().read(cx).as_singleton() else {
        return;
    };
    let Some(addon) = editor.addon::<BufferDecorationAddon>() else {
        return;
    };
    let registry = addon.registry.clone();

    let language = buffer
        .read(cx)
        .language()
        .map(|language| language.name().to_string());
    let decorator =
        language.and_then(|language| registry.read(cx).decorator_for_language(&language));
    let Some(decorator) = decorator else {
        // Also drop a call to the previous decorator that is still running.
        if let Some(addon) = editor.addon_mut::<BufferDecorationAddon>() {
            addon.refresh = Task::ready(());
        }
        clear(editor, cx);
        return;
    };

    let refresh = cx.spawn(async move |editor, cx| {
        cx.background_executor().timer(DECORATE_DEBOUNCE).await;

        let Some((text, version)) = buffer.read_with(cx, |buffer, _| {
            (buffer.len() <= MAX_DECORATED_TEXT_BYTES).then(|| (buffer.text(), buffer.version()))
        }) else {
            editor.update(cx, |editor, cx| clear(editor, cx)).ok();
            return;
        };

        let decorations = match decorator
            .extension
            .decorate_buffer(decorator.id.clone(), text)
            .await
        {
            Ok(decorations) => decorations,
            Err(error) => {
                log::error!("buffer decorator {} failed: {error:#}", decorator.id);
                return;
            }
        };

        editor
            .update(cx, |editor, cx| {
                // The text moved on while the decorator ran: its offsets no
                // longer fit, and the edit that moved it has scheduled a newer call.
                let is_current = buffer.read(cx).version() == version;
                if is_current {
                    set_decorations(editor, decorations, cx);
                }
            })
            .ok();
    });

    if let Some(addon) = editor.addon_mut::<BufferDecorationAddon>() {
        addon.refresh = refresh;
    }
}

/// Turns the decorator's byte offsets, which fit the current text, into anchors,
/// and applies them.
fn set_decorations(editor: &mut Editor, decorations: BufferDecorations, cx: &mut Context<Editor>) {
    let snapshot = editor.buffer().read(cx).snapshot(cx);
    let anchor = |offset: usize| snapshot.anchor_after(MultiBufferOffset(offset));
    // Text typed right at either end stays outside the range.
    let anchors = |range: &Range<usize>| {
        anchor(range.start)..snapshot.anchor_before(MultiBufferOffset(range.end))
    };

    let applied = AppliedDecorations {
        styles: decorations
            .styles
            .iter()
            .map(|styled| (styled.style, anchors(&styled.range)))
            .collect(),
        concealments: decorations
            .concealments
            .iter()
            .map(|concealment| AnchoredConcealment {
                range: anchors(&concealment.range),
                replacement: concealment.replacement.clone().map(Into::into),
                reveal: anchors(&concealment.reveal),
            })
            .collect(),
    };

    if let Some(addon) = editor.addon_mut::<BufferDecorationAddon>() {
        addon.applied = applied;
    }
    apply(editor, cx);
}

/// Draws the last decorations: styles as text highlights, and the concealments
/// that no cursor or selection touches.
fn apply(editor: &mut Editor, cx: &mut Context<Editor>) {
    let Some(addon) = editor.addon::<BufferDecorationAddon>() else {
        return;
    };
    let styles = addon.applied.styles.clone();
    let concealments = addon.applied.concealments.clone();

    let snapshot = editor.buffer().read(cx).snapshot(cx);
    let selections: Vec<Range<usize>> = editor
        .selections
        .disjoint_anchors()
        .iter()
        .map(|selection| {
            selection.start.to_offset(&snapshot).0..selection.end.to_offset(&snapshot).0
        })
        .collect();

    let hidden = concealments
        .into_iter()
        .filter(|concealment| {
            let reveal = concealment.reveal.start.to_offset(&snapshot).0
                ..concealment.reveal.end.to_offset(&snapshot).0;
            !is_touched(&reveal, &selections)
        })
        .map(|concealment| Concealment {
            range: concealment.range,
            replacement: concealment.replacement,
        })
        .collect();
    editor.set_concealments(hidden, cx);

    for (index, text_style) in style::ALL_STYLES.iter().enumerate() {
        let ranges: Vec<_> = styles
            .iter()
            .filter(|(style, _)| style == text_style)
            .map(|(_, range)| range.clone())
            .collect();
        let key = HighlightKey::BufferDecoration(index);
        if ranges.is_empty() {
            editor.clear_highlights(key, cx);
        } else {
            let style = highlight_style(*text_style, cx);
            editor.highlight_text(key, ranges, style, cx);
        }
    }
}

/// Removes everything this crate drew.
fn clear(editor: &mut Editor, cx: &mut Context<Editor>) {
    if let Some(addon) = editor.addon_mut::<BufferDecorationAddon>() {
        addon.applied = AppliedDecorations::default();
    }
    apply(editor, cx);
}

/// Whether a cursor or selection touches `reveal`: it overlaps it, or sits
/// right at its start or end.
fn is_touched(reveal: &Range<usize>, selections: &[Range<usize>]) -> bool {
    selections
        .iter()
        .any(|selection| selection.start <= reveal.end && selection.end >= reveal.start)
}

#[cfg(test)]
mod reveal_tests {
    use super::is_touched;

    #[test]
    fn cursor_touching_the_token_reveals_it() {
        let token = 2..9;
        // Cursor at both edges and inside the token.
        assert!(is_touched(&token, &[2..2]));
        assert!(is_touched(&token, &[9..9]));
        assert!(is_touched(&token, &[5..5]));
        // Cursor away from the token.
        assert!(!is_touched(&token, &[1..1]));
        assert!(!is_touched(&token, &[10..10]));
    }

    #[test]
    fn selection_overlapping_the_token_reveals_it() {
        let token = 4..8;
        assert!(is_touched(&token, &[0..5]));
        assert!(is_touched(&token, &[7..20]));
        assert!(is_touched(&token, &[0..20]));
        assert!(!is_touched(&token, &[0..3]));
    }

    #[test]
    fn any_of_several_cursors_reveals_it() {
        let token = 4..8;
        assert!(is_touched(&token, &[0..0, 6..6]));
        assert!(!is_touched(&token, &[0..0, 12..12]));
    }
}

#[cfg(test)]
mod concealment_tests {
    use editor::{
        Concealment, Editor, MultiBufferOffset, test::editor_test_context::EditorTestContext,
    };
    use gpui::TestAppContext;
    use settings::SettingsStore;

    fn init_test(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let settings = SettingsStore::test(cx);
            cx.set_global(settings);
            theme_settings::init(theme::LoadThemes::JustBase, cx);
            editor::init(cx);
        });
    }

    fn conceal(cx: &mut EditorTestContext, concealments: &[(usize, usize, Option<&str>)]) {
        cx.update_editor(|editor: &mut Editor, _, cx| {
            let snapshot = editor.buffer().read(cx).snapshot(cx);
            let concealments = concealments
                .iter()
                .map(|(start, end, replacement)| Concealment {
                    range: snapshot.anchor_after(MultiBufferOffset(*start))
                        ..snapshot.anchor_after(MultiBufferOffset(*end)),
                    replacement: replacement.map(|text| text.to_string().into()),
                })
                .collect();
            editor.set_concealments(concealments, cx);
        });
    }

    #[gpui::test]
    async fn concealments_hide_or_replace_text_without_changing_the_buffer(
        cx: &mut TestAppContext,
    ) {
        init_test(cx);
        let mut cx = EditorTestContext::new(cx).await;
        cx.set_state("ˇ- item **bold** end");

        conceal(&mut cx, &[(0, 1, Some("•")), (7, 9, None), (13, 15, None)]);
        assert_eq!(cx.display_text(), "• item bold end");
        assert_eq!(cx.buffer_text(), "- item **bold** end");

        // The same set again changes nothing.
        conceal(&mut cx, &[(0, 1, Some("•")), (7, 9, None), (13, 15, None)]);
        assert_eq!(cx.display_text(), "• item bold end");

        // A smaller set shows the dropped ranges again.
        conceal(&mut cx, &[(0, 1, Some("•"))]);
        assert_eq!(cx.display_text(), "• item **bold** end");

        // An empty set clears them all.
        conceal(&mut cx, &[]);
        assert_eq!(cx.display_text(), "- item **bold** end");
    }

    #[gpui::test]
    async fn a_changed_replacement_is_redrawn(cx: &mut TestAppContext) {
        init_test(cx);
        let mut cx = EditorTestContext::new(cx).await;
        cx.set_state("ˇ- a");

        conceal(&mut cx, &[(0, 1, Some("•"))]);
        assert_eq!(cx.display_text(), "• a");
        conceal(&mut cx, &[(0, 1, Some("◦"))]);
        assert_eq!(cx.display_text(), "◦ a");
        conceal(&mut cx, &[(0, 1, None)]);
        assert_eq!(cx.display_text(), " a");
    }

    #[gpui::test]
    async fn concealments_follow_edits_before_them(cx: &mut TestAppContext) {
        init_test(cx);
        let mut cx = EditorTestContext::new(cx).await;
        cx.set_state("ˇ**b**");

        conceal(&mut cx, &[(0, 2, None), (3, 5, None)]);
        assert_eq!(cx.display_text(), "b");

        cx.update_multibuffer(|buffer, cx| {
            buffer.edit(
                [(MultiBufferOffset(0)..MultiBufferOffset(0), "xx ")],
                None,
                cx,
            );
        });
        assert_eq!(cx.buffer_text(), "xx **b**");
        // The anchors moved with the text, so the markers stay hidden.
        assert_eq!(cx.display_text(), "xx b");
    }
}
