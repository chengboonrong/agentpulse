//! A single-line text field: typing with IME (marked text), deletion, caret and
//! shift-selection moves by character, select all, copy, cut and paste, click to
//! place the caret. Enter submits, Escape reverts to the saved value, Tab moves on.
//! A secure field shows bullets, never copies or cuts, and turns on Secure Event
//! Input while it has focus.
//!
//! Adapted from Zed's `crates/gpui/examples/input.rs` at the pinned rev
//! 27bbbc1cc480a7500cec122f120e796d1fb8500c.
//! Copyright 2022 - 2025 Zed Industries, Inc.
//! Licensed under the Apache License, Version 2.0
//! (http://www.apache.org/licenses/LICENSE-2.0).
//! Changes: the editing state is a plain [`Editor`] tested without a window,
//! moves are by `char` rather than grapheme (no new crate), plus secure mode,
//! deleting to the start or by word, horizontal scrolling, submit and revert,
//! and this app's theme.

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, KeyBinding,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point,
    ShapedLine, SharedString, Style, Subscription, TextRun, UTF16Selection, UnderlineStyle, Window,
    actions, div, fill, point, prelude::*, px, relative, rgb, rgba, size,
};

use super::{DARK, LIGHT, Shared, is_dark, sp};

actions!(
    text_field,
    [
        Backspace,
        DeleteToStart,
        DeleteWord,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        SelectHome,
        SelectEnd,
        Paste,
        Cut,
        Copy,
        Submit,
        Revert,
        ShowCharacterPalette,
    ]
);

const CONTEXT: &str = "TextField";
const BULLET: char = '•';

/// The field's keys, active only while a field has focus.
pub(super) fn bindings() -> Vec<KeyBinding> {
    let c = Some(CONTEXT);
    vec![
        KeyBinding::new("backspace", Backspace, c),
        KeyBinding::new("cmd-backspace", DeleteToStart, c),
        KeyBinding::new("alt-backspace", DeleteWord, c),
        KeyBinding::new("delete", Delete, c),
        KeyBinding::new("left", Left, c),
        KeyBinding::new("right", Right, c),
        KeyBinding::new("shift-left", SelectLeft, c),
        KeyBinding::new("shift-right", SelectRight, c),
        KeyBinding::new("cmd-a", SelectAll, c),
        KeyBinding::new("home", Home, c),
        KeyBinding::new("cmd-left", Home, c),
        KeyBinding::new("end", End, c),
        KeyBinding::new("cmd-right", End, c),
        KeyBinding::new("shift-home", SelectHome, c),
        KeyBinding::new("shift-cmd-left", SelectHome, c),
        KeyBinding::new("shift-end", SelectEnd, c),
        KeyBinding::new("shift-cmd-right", SelectEnd, c),
        KeyBinding::new("cmd-v", Paste, c),
        KeyBinding::new("cmd-x", Cut, c),
        KeyBinding::new("cmd-c", Copy, c),
        KeyBinding::new("enter", Submit, c),
        KeyBinding::new("escape", Revert, c),
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, c),
    ]
}

/// The editing state, in byte offsets into `text`, always on `char` boundaries.
#[derive(Clone, Default, PartialEq, Eq)]
pub(super) struct Editor {
    text: String,
    selected: Range<usize>,
    /// The caret is at the selection's start.
    reversed: bool,
    /// IME composition in progress.
    marked: Option<Range<usize>>,
    secure: bool,
}

/// A secure field's text never reaches a log or a panic message.
impl std::fmt::Debug for Editor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = if self.secure {
            format!("<{} chars hidden>", self.text.chars().count())
        } else {
            format!("{:?}", self.text)
        };
        f.debug_struct("Editor")
            .field("text", &format_args!("{text}"))
            .field("selected", &self.selected)
            .field("reversed", &self.reversed)
            .field("marked", &self.marked)
            .field("secure", &self.secure)
            .finish()
    }
}

impl Editor {
    pub fn new(secure: bool) -> Self {
        Self {
            secure,
            ..Self::default()
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Replaces the whole text, caret at the end.
    pub fn set_text(&mut self, text: &str) {
        self.text = one_line(text);
        self.selected = self.text.len()..self.text.len();
        self.reversed = false;
        self.marked = None;
    }

    pub fn cursor(&self) -> usize {
        if self.reversed {
            self.selected.start
        } else {
            self.selected.end
        }
    }

    fn prev(&self, at: usize) -> usize {
        self.text[..at]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    fn next(&self, at: usize) -> usize {
        self.text[at..]
            .chars()
            .next()
            .map_or(self.text.len(), |c| at + c.len_utf8())
    }

    pub fn move_to(&mut self, at: usize) {
        self.selected = at..at;
        self.reversed = false;
    }

    pub fn select_to(&mut self, at: usize) {
        if self.reversed {
            self.selected.start = at;
        } else {
            self.selected.end = at;
        }
        if self.selected.end < self.selected.start {
            self.reversed = !self.reversed;
            self.selected = self.selected.end..self.selected.start;
        }
    }

    pub fn left(&mut self) {
        if self.selected.is_empty() {
            self.move_to(self.prev(self.cursor()));
        } else {
            self.move_to(self.selected.start);
        }
    }

    pub fn right(&mut self) {
        if self.selected.is_empty() {
            self.move_to(self.next(self.cursor()));
        } else {
            self.move_to(self.selected.end);
        }
    }

    pub fn select_left(&mut self) {
        self.select_to(self.prev(self.cursor()));
    }

    pub fn select_right(&mut self) {
        self.select_to(self.next(self.cursor()));
    }

    pub fn select_all(&mut self) {
        self.move_to(0);
        self.select_to(self.text.len());
    }

    /// Deletes the selection or the character before the caret; false when there is neither.
    pub fn backspace(&mut self) -> bool {
        self.delete_back_to(self.prev(self.cursor()))
    }

    /// Deletes the selection or everything before the caret (cmd-backspace).
    pub fn delete_to_start(&mut self) -> bool {
        self.delete_back_to(0)
    }

    /// Deletes the selection or the word before the caret, with the spaces and
    /// punctuation after it (alt-backspace). A secure field has no words to
    /// reveal, so it deletes to the start, as macOS's secure fields do.
    pub fn delete_word(&mut self) -> bool {
        let at = if self.secure {
            0
        } else {
            let before = &self.text[..self.cursor()];
            let gap = before.trim_end_matches(|c: char| !c.is_alphanumeric());
            gap.trim_end_matches(char::is_alphanumeric).len()
        };
        self.delete_back_to(at)
    }

    fn delete_back_to(&mut self, at: usize) -> bool {
        if self.selected.is_empty() {
            if self.cursor() == 0 {
                return false;
            }
            self.select_to(at);
        }
        self.replace(None, "");
        true
    }

    /// Deletes the selection or the character after the caret; false when there is neither.
    pub fn delete(&mut self) -> bool {
        if self.selected.is_empty() {
            if self.cursor() == self.text.len() {
                return false;
            }
            self.select_to(self.next(self.cursor()));
        }
        self.replace(None, "");
        true
    }

    /// The selection to put on the clipboard; never for a secure field.
    pub fn copy(&self) -> Option<String> {
        (!self.secure && !self.selected.is_empty()).then(|| self.text[self.selected.clone()].into())
    }

    /// Removes and returns the selection; a secure field neither copies nor deletes.
    pub fn cut(&mut self) -> Option<String> {
        let text = self.copy()?;
        self.replace(None, "");
        Some(text)
    }

    /// Replaces `range` (else the marked text, else the selection) with `new`,
    /// caret after it. Line breaks become spaces.
    pub fn replace(&mut self, range: Option<Range<usize>>, new: &str) {
        let range = self.target(range);
        let new = one_line(new);
        self.text.replace_range(range.clone(), &new);
        self.move_to(range.start + new.len());
        self.marked = None;
    }

    /// As [`Self::replace`], leaving `new` marked as IME composition with the
    /// selection at `selected` within it (byte offsets into `new`).
    pub fn replace_and_mark(
        &mut self,
        range: Option<Range<usize>>,
        new: &str,
        selected: Option<Range<usize>>,
    ) {
        // A secure field shows no composition: what the input method sends goes straight in.
        if self.secure {
            self.replace(range, new);
            return;
        }
        let range = self.target(range);
        let new = one_line(new);
        self.text.replace_range(range.clone(), &new);
        self.marked = (!new.is_empty()).then(|| range.start..range.start + new.len());
        let clamp = |i: usize| range.start + floor_char(&new, i.min(new.len()));
        self.selected = selected.map_or(range.start + new.len()..range.start + new.len(), |s| {
            clamp(s.start)..clamp(s.end)
        });
        self.reversed = false;
    }

    fn target(&self, range: Option<Range<usize>>) -> Range<usize> {
        range
            .or(self.marked.clone())
            .unwrap_or(self.selected.clone())
    }

    /// The text in `range` for the input system: a bullet per UTF-16 unit when
    /// secure, so a secure field never hands its text out.
    pub fn text_for_input(&self, range: Range<usize>) -> String {
        let text = &self.text[range];
        if self.secure {
            text.encode_utf16().map(|_| BULLET).collect()
        } else {
            text.into()
        }
    }

    /// What is drawn: the text, or a bullet per character when secure.
    pub fn display(&self) -> String {
        if self.secure {
            self.text.chars().map(|_| BULLET).collect()
        } else {
            self.text.clone()
        }
    }

    /// A text offset in the drawn text.
    pub fn to_display(&self, at: usize) -> usize {
        if self.secure {
            self.text[..at].chars().count() * BULLET.len_utf8()
        } else {
            at
        }
    }

    /// A drawn-text offset in the text.
    pub fn display_to_text(&self, at: usize) -> usize {
        if self.secure {
            let n = at / BULLET.len_utf8();
            self.text
                .char_indices()
                .nth(n)
                .map_or(self.text.len(), |(i, _)| i)
        } else {
            floor_char(&self.text, at.min(self.text.len()))
        }
    }

    pub fn utf16_to_text(&self, at: usize) -> usize {
        let mut units = 0;
        for (i, c) in self.text.char_indices() {
            if units >= at {
                return i;
            }
            units += c.len_utf16();
        }
        self.text.len()
    }

    pub fn to_utf16(&self, at: usize) -> usize {
        self.text[..at].encode_utf16().count()
    }

    fn range_from_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.utf16_to_text(r.start)..self.utf16_to_text(r.end)
    }

    fn range_to_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.to_utf16(r.start)..self.to_utf16(r.end)
    }
}

fn one_line(s: &str) -> String {
    s.replace(['\r', '\n'], " ")
}

fn floor_char(s: &str, mut at: usize) -> usize {
    while !s.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// Enter was pressed.
pub(super) struct Submitted;

pub(super) struct TextField {
    editor: Editor,
    focus: FocusHandle,
    label: SharedString,
    placeholder: SharedString,
    /// What Escape goes back to: the stored value.
    saved: String,
    /// Drawn-text x of the field's left edge, so the caret stays in view.
    scroll: Pixels,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    selecting: bool,
    /// This secure field turned Secure Event Input on and has not turned it off yet.
    secure_input: bool,
    _subscriptions: Vec<Subscription>,
}

impl Drop for TextField {
    fn drop(&mut self) {
        if self.secure_input {
            agentpulse_platform_macos::secure_event_input(false);
        }
    }
}

impl EventEmitter<Submitted> for TextField {}

impl Focusable for TextField {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TextField {
    pub fn new(
        label: &'static str,
        placeholder: &'static str,
        secure: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle().tab_stop(true);
        let subscriptions = if secure {
            vec![
                cx.on_focus(&focus, window, |f, w, _| f.sync_secure_input(w)),
                cx.on_blur(&focus, window, |f, w, _| f.sync_secure_input(w)),
                cx.observe_window_activation(window, |f, w, _| f.sync_secure_input(w)),
            ]
        } else {
            vec![]
        };
        Self {
            editor: Editor::new(secure),
            focus,
            label: label.into(),
            placeholder: placeholder.into(),
            saved: String::new(),
            scroll: px(0.),
            last_layout: None,
            last_bounds: None,
            selecting: false,
            secure_input: false,
            _subscriptions: subscriptions,
        }
    }

    /// Secure Event Input on exactly while this secure field has focus in the
    /// active window; each on is matched by one off (here or on drop).
    fn sync_secure_input(&mut self, window: &Window) {
        let want = self.editor.secure && window.is_window_active() && self.focus.is_focused(window);
        if want != self.secure_input {
            agentpulse_platform_macos::secure_event_input(want);
            self.secure_input = want;
        }
    }

    pub fn text(&self) -> &str {
        self.editor.text()
    }

    /// The stored value changed: shown unless the text has been edited since.
    pub fn set_saved(&mut self, saved: &str, cx: &mut Context<Self>) {
        if self.saved == saved {
            return;
        }
        if self.editor.text() == self.saved {
            self.editor.set_text(saved);
        }
        self.saved = saved.into();
        cx.notify();
    }

    /// Shows `saved` as the stored value, dropping any edit.
    pub fn reset(&mut self, saved: &str, cx: &mut Context<Self>) {
        self.saved = saved.into();
        self.editor.set_text(saved);
        cx.notify();
    }

    fn edited(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.notify_text_input_changed(&self.focus);
        cx.notify();
    }

    fn moved(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.notify_text_selection_changed(&self.focus);
        cx.notify();
    }

    fn index_at(&self, position: Point<Pixels>) -> usize {
        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };
        if self.editor.text().is_empty() {
            return 0;
        }
        let x = position.x - bounds.left() + self.scroll;
        self.editor.display_to_text(line.closest_index_for_x(x))
    }

    fn on_mouse_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.selecting = true;
        let at = self.index_at(e.position);
        if e.modifiers.shift {
            self.editor.select_to(at);
        } else {
            self.editor.move_to(at);
        }
        self.moved(window, cx);
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }

    fn on_mouse_move(&mut self, e: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.selecting {
            self.editor.select_to(self.index_at(e.position));
            self.moved(window, cx);
        }
    }
}

impl EntityInputHandler for TextField {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let e = &self.editor;
        let range = e.range_from_utf16(&range_utf16);
        actual_range.replace(e.range_to_utf16(&range));
        Some(e.text_for_input(range))
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.editor.range_to_utf16(&self.editor.selected),
            reversed: self.editor.reversed,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.editor
            .marked
            .as_ref()
            .map(|r| self.editor.range_to_utf16(r))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.marked = None;
        self.edited(window, cx);
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16.map(|r| self.editor.range_from_utf16(&r));
        self.editor.replace(range, new_text);
        self.edited(window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16.map(|r| self.editor.range_from_utf16(&r));
        // The selection arrives in UTF-16 units of `new_text`.
        let units = |at: usize| {
            let mut n = 0;
            new_text
                .char_indices()
                .find(|(_, c)| {
                    n += c.len_utf16();
                    n > at
                })
                .map_or(new_text.len(), |(i, _)| i)
        };
        let selected = new_selected_range_utf16.map(|r| units(r.start)..units(r.end));
        self.editor.replace_and_mark(range, new_text, selected);
        self.edited(window, cx);
    }

    fn set_selected_text_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor.selected = self.editor.range_from_utf16(&range_utf16);
        self.editor.reversed = false;
        self.moved(window, cx);
    }

    fn text_length_utf16(&mut self, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        Some(self.editor.to_utf16(self.editor.text().len()))
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let line = self.last_layout.as_ref()?;
        let range = self.editor.range_from_utf16(&range_utf16);
        let x = |at| bounds.left() - self.scroll + line.x_for_index(self.editor.to_display(at));
        Some(Bounds::from_corners(
            point(x(range.start), bounds.top()),
            point(x(range.end), bounds.bottom()),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let local = self.last_bounds?.localize(&point)?;
        let line = self.last_layout.as_ref()?;
        let at = self
            .editor
            .display_to_text(line.closest_index_for_x(local.x + self.scroll));
        Some(self.editor.to_utf16(at))
    }
}

impl Render for TextField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dark = cx
            .try_global::<Shared>()
            .and_then(|s| s.appearance)
            .unwrap_or(is_dark(window.appearance()));
        let t = if dark { DARK } else { LIGHT };
        div()
            .id(ElementId::Name(self.label.clone()))
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .role(if self.editor.secure {
                gpui::Role::PasswordInput
            } else {
                gpui::Role::TextInput
            })
            .aria_label(self.label.clone())
            // The value VoiceOver reads: the text, or bullets for a secure field.
            .aria_value(self.editor.display())
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(|f, _: &Backspace, w, cx| {
                if f.editor.backspace() {
                    f.edited(w, cx);
                } else {
                    w.play_system_bell();
                }
            }))
            .on_action(cx.listener(|f, _: &DeleteToStart, w, cx| {
                if f.editor.delete_to_start() {
                    f.edited(w, cx);
                } else {
                    w.play_system_bell();
                }
            }))
            .on_action(cx.listener(|f, _: &DeleteWord, w, cx| {
                if f.editor.delete_word() {
                    f.edited(w, cx);
                } else {
                    w.play_system_bell();
                }
            }))
            .on_action(cx.listener(|f, _: &Delete, w, cx| {
                if f.editor.delete() {
                    f.edited(w, cx);
                } else {
                    w.play_system_bell();
                }
            }))
            .on_action(cx.listener(|f, _: &Left, w, cx| {
                f.editor.left();
                f.moved(w, cx);
            }))
            .on_action(cx.listener(|f, _: &Right, w, cx| {
                f.editor.right();
                f.moved(w, cx);
            }))
            .on_action(cx.listener(|f, _: &SelectLeft, w, cx| {
                f.editor.select_left();
                f.moved(w, cx);
            }))
            .on_action(cx.listener(|f, _: &SelectRight, w, cx| {
                f.editor.select_right();
                f.moved(w, cx);
            }))
            .on_action(cx.listener(|f, _: &SelectAll, w, cx| {
                f.editor.select_all();
                f.moved(w, cx);
            }))
            .on_action(cx.listener(|f, _: &Home, w, cx| {
                f.editor.move_to(0);
                f.moved(w, cx);
            }))
            .on_action(cx.listener(|f, _: &End, w, cx| {
                f.editor.move_to(f.editor.text().len());
                f.moved(w, cx);
            }))
            .on_action(cx.listener(|f, _: &SelectHome, w, cx| {
                f.editor.select_to(0);
                f.moved(w, cx);
            }))
            .on_action(cx.listener(|f, _: &SelectEnd, w, cx| {
                f.editor.select_to(f.editor.text().len());
                f.moved(w, cx);
            }))
            .on_action(cx.listener(|f, _: &Paste, w, cx| {
                if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    f.editor.replace(None, &text);
                    f.edited(w, cx);
                }
            }))
            .on_action(cx.listener(|f, _: &Copy, _, cx| {
                if let Some(text) = f.editor.copy() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
            }))
            .on_action(cx.listener(|f, _: &Cut, w, cx| {
                if let Some(text) = f.editor.cut() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                    f.edited(w, cx);
                }
            }))
            .on_action(cx.listener(|_, _: &Submit, _, cx| cx.emit(Submitted)))
            .on_action(cx.listener(|f, _: &Revert, w, cx| {
                let saved = f.saved.clone();
                f.editor.set_text(&saved);
                f.edited(w, cx);
            }))
            .on_action(cx.listener(|_, _: &ShowCharacterPalette, w, _| {
                w.show_character_palette();
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .flex_1()
            .min_w_0()
            .px(px(sp::S))
            .py(px(sp::XS))
            .rounded(px(8.))
            .border_1()
            // Muted, not the keyline: at least 3:1 against the card and the
            // field's own fill in both themes (the keyline was about 1.2:1).
            .border_color(rgb(t.muted))
            .bg(rgb(t.window))
            .focus(|s| s.border_color(rgb(t.text)))
            .overflow_hidden()
            .line_height(px(18.))
            .child(TextElement {
                field: cx.entity(),
                caret: t.text,
                placeholder: t.muted,
                selection: t.selected,
            })
    }
}

struct TextElement {
    field: Entity<TextField>,
    caret: u32,
    placeholder: u32,
    selection: u32,
}

struct Prepaint {
    line: Option<ShapedLine>,
    caret: Option<PaintQuad>,
    selection: Option<PaintQuad>,
    scroll: Pixels,
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Prepaint {
        let field = self.field.read(cx);
        let e = &field.editor;
        let focused = field.focus.is_focused(window);
        let style = window.text_style();
        let (text, color): (SharedString, _) = if e.text().is_empty() {
            (field.placeholder.clone(), rgb(self.placeholder).into())
        } else {
            (e.display().into(), style.color)
        };
        let run = TextRun {
            len: text.len(),
            font: style.font(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = match e.marked.as_ref().filter(|_| !e.text().is_empty()) {
            Some(m) => {
                let m = e.to_display(m.start)..e.to_display(m.end);
                vec![
                    TextRun {
                        len: m.start,
                        ..run.clone()
                    },
                    TextRun {
                        len: m.end - m.start,
                        underline: Some(UnderlineStyle {
                            color: Some(run.color),
                            thickness: px(1.),
                            wavy: false,
                        }),
                        ..run.clone()
                    },
                    TextRun {
                        len: text.len() - m.end,
                        ..run
                    },
                ]
                .into_iter()
                .filter(|r| r.len > 0)
                .collect()
            }
            None => vec![run],
        };
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(text, font_size, &runs, None);
        let x = |at: usize| {
            if e.text().is_empty() {
                px(0.)
            } else {
                line.x_for_index(e.to_display(at))
            }
        };
        // Keep the caret inside the field, scrolling no further than needed.
        let caret_x = x(e.cursor());
        let room = (bounds.size.width - px(2.)).max(px(0.));
        let scroll = field
            .scroll
            .min(caret_x)
            .max(caret_x - room)
            .min((line.width - room).max(px(0.)))
            .max(px(0.));
        let left = bounds.left() - scroll;
        let (selection, caret) = if e.selected.is_empty() || !focused {
            let caret = fill(
                Bounds::new(
                    point(left + caret_x, bounds.top()),
                    size(px(1.5), bounds.size.height),
                ),
                rgb(self.caret),
            );
            (None, Some(caret))
        } else {
            let sel = fill(
                Bounds::from_corners(
                    point(left + x(e.selected.start), bounds.top()),
                    point(left + x(e.selected.end), bounds.bottom()),
                ),
                rgba(self.selection << 8 | 0xFF),
            );
            (Some(sel), None)
        };
        Prepaint {
            line: Some(line),
            caret,
            selection,
            scroll,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut Prepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.field.read(cx).focus.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.field.clone()),
            cx,
        );
        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection);
        }
        let Some(line) = prepaint.line.take() else {
            return;
        };
        line.paint(
            point(bounds.left() - prepaint.scroll, bounds.top()),
            window.line_height(),
            gpui::TextAlign::Left,
            None,
            window,
            cx,
        )
        .ok();
        if focus.is_focused(window)
            && let Some(caret) = prepaint.caret.take()
        {
            window.paint_quad(caret);
        }
        let scroll = prepaint.scroll;
        self.field.update(cx, |f, _| {
            f.last_layout = Some(line);
            f.last_bounds = Some(bounds);
            f.scroll = scroll;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(text: &str) -> Editor {
        let mut e = Editor::new(false);
        e.set_text(text);
        e
    }

    #[test]
    fn typing_inserts_at_the_caret_and_replaces_the_selection() {
        let mut e = editor("");
        e.replace(None, "https://relay");
        assert_eq!((e.text(), e.cursor()), ("https://relay", 13));
        e.move_to(8);
        e.replace(None, "my-");
        assert_eq!(e.text(), "https://my-relay");
        e.select_all();
        e.replace(None, "x\ny");
        assert_eq!((e.text(), e.cursor()), ("x y", 3), "one line only");
    }

    #[test]
    fn backspace_and_delete_take_one_character_or_the_selection() {
        let mut e = editor("aé🙂b");
        assert!(e.backspace());
        assert_eq!(e.text(), "aé🙂");
        assert!(e.backspace());
        assert_eq!(e.text(), "aé", "a whole multi-byte character");
        e.move_to(0);
        assert!(!e.backspace(), "nothing before the caret");
        assert!(e.delete());
        assert_eq!(e.text(), "é");
        e.move_to(e.text().len());
        assert!(!e.delete(), "nothing after the caret");
        let mut e = editor("abcdef");
        e.move_to(1);
        e.select_to(4);
        assert!(e.delete());
        assert_eq!((e.text(), e.cursor()), ("aef", 1));
    }

    #[test]
    fn caret_moves_by_character_and_shift_extends_the_selection() {
        let mut e = editor("ab🙂");
        e.left();
        assert_eq!(e.cursor(), 2, "over the 4-byte emoji");
        e.left();
        e.left();
        e.left();
        assert_eq!(e.cursor(), 0, "stops at the start");
        e.right();
        assert_eq!(e.cursor(), 1);
        e.select_right();
        e.select_right();
        assert_eq!((e.selected.clone(), e.cursor()), (1..6, 6));
        // Back past the anchor: the selection flips to the caret's side.
        e.select_left();
        e.select_left();
        e.select_left();
        assert_eq!((e.selected.clone(), e.cursor()), (0..1, 0));
        // Left with a selection collapses to its start; right to its end.
        e.select_all();
        e.left();
        assert_eq!(e.selected, 0..0);
        e.select_all();
        e.right();
        assert_eq!(e.selected, 6..6);
        e.select_to(0);
        assert_eq!((e.selected.clone(), e.reversed), (0..6, true), "shift-home");
    }

    #[test]
    fn copy_and_cut_take_the_selection() {
        let mut e = editor("relay.example.com");
        assert_eq!(e.copy(), None, "nothing selected");
        e.move_to(0);
        e.select_to(5);
        assert_eq!(e.copy().as_deref(), Some("relay"));
        assert_eq!(e.cut().as_deref(), Some("relay"));
        assert_eq!(e.text(), ".example.com");
    }

    #[test]
    fn a_secure_field_never_copies_or_cuts_and_draws_bullets() {
        let mut e = Editor::new(true);
        e.replace(None, "s3cr€t");
        e.select_all();
        assert_eq!(e.copy(), None);
        assert_eq!(e.cut(), None);
        assert_eq!(e.text(), "s3cr€t", "cut left the text alone");
        assert_eq!(e.display(), "••••••");
        // The euro sign is 3 bytes, the same as a bullet.
        assert_eq!(e.to_display(e.text().len()), 18);
        assert_eq!(e.display_to_text(12), 4);
        assert_eq!(e.display_to_text(18), e.text().len());
    }

    #[test]
    fn cmd_and_alt_backspace_delete_to_the_start_and_by_word() {
        let mut e = editor("https://relay.example.com/x");
        assert!(e.delete_word());
        assert_eq!(e.text(), "https://relay.example.com/");
        assert!(e.delete_word(), "the slash and the word before it");
        assert_eq!(e.text(), "https://relay.example.");
        e.move_to(8);
        assert!(e.delete_to_start());
        assert_eq!((e.text(), e.cursor()), ("relay.example.", 0));
        assert!(
            !e.delete_to_start() && !e.delete_word(),
            "nothing before the caret"
        );
        // With a selection, both delete just the selection.
        e.move_to(0);
        e.select_to(6);
        assert!(e.delete_word());
        assert_eq!(e.text(), "example.");
        // A secure field has no words: alt-backspace clears to the start.
        let mut e = Editor::new(true);
        e.replace(None, "two words");
        assert!(e.delete_word());
        assert_eq!(e.text(), "");
    }

    #[test]
    fn a_secure_field_hides_its_text_from_input_and_debug() {
        let mut e = Editor::new(true);
        e.replace(None, "k€y🙂");
        assert_eq!(
            e.text_for_input(0..e.text().len()),
            "•••••",
            "one per UTF-16 unit"
        );
        assert!(!format!("{e:?}").contains("k€y"));
        assert!(format!("{e:?}").contains("4 chars hidden"));
        assert!(format!("{:?}", editor("relay")).contains("\"relay\""));
        assert_eq!(editor("relay").text_for_input(1..3), "el");
        // Input-method composition goes straight in: nothing stays marked.
        e.replace_and_mark(None, "あ", Some(0..3));
        assert_eq!((e.text(), e.marked.clone()), ("k€y🙂あ", None));
    }

    #[test]
    fn ime_marked_text_is_replaced_until_committed() {
        let mut e = editor("A🙂Z");
        // Composition replacing the emoji, selection inside the new text.
        e.replace_and_mark(Some(1..5), "é🙂", Some(2..6));
        assert_eq!(e.text(), "Aé🙂Z");
        assert_eq!(e.marked, Some(1..7));
        assert_eq!(e.selected, 3..7);
        // The next composition replaces the marked text.
        e.replace_and_mark(None, "漢", None);
        assert_eq!((e.text(), e.marked.clone()), ("A漢Z", Some(1..4)));
        // Committing replaces it and ends the composition.
        e.replace(None, "字");
        assert_eq!((e.text(), e.marked.clone(), e.cursor()), ("A字Z", None, 4));
    }

    #[test]
    fn utf16_offsets_map_to_byte_offsets() {
        let e = editor("e🙂漢");
        assert_eq!(e.to_utf16(e.text().len()), 4);
        assert_eq!(e.utf16_to_text(1), 1);
        assert_eq!(e.utf16_to_text(3), 5);
        assert_eq!(e.utf16_to_text(99), e.text().len());
    }
}
