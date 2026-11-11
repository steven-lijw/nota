//! A plain-text input (single paragraph or multi-line) with selection,
//! clipboard and IME support. Used for titles, search, table cells and the
//! AI prompt.

use crate::actions::*;
use crate::edit::*;
use crate::text_view::{TextLayout, TextView};
use crate::theme::{theme, ui_font};
use gpui::{
    div, font, prelude::*, px, App, Bounds, ClipboardItem, Context, ElementInputHandler, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, FontWeight, Hsla, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, SharedString, Subscription, TextRun,
    UTF16Selection, Window,
};
use std::ops::Range;

#[derive(Clone, Debug, PartialEq)]
pub enum InputEvent {
    Changed,
    Submit,
    Cancel,
    Blur,
    /// Arrow-up on the first row / arrow-down on the last row.
    MoveUp,
    MoveDown,
    /// Backspace pressed while empty.
    BackspaceEmpty,
}

pub struct TextInput {
    focus: FocusHandle,
    text: String,
    sel: Range<usize>,
    reversed: bool,
    marked: Option<Range<usize>>,
    layout: Option<TextLayout>,
    selecting: bool,
    pub placeholder: SharedString,
    pub font_size: Pixels,
    pub line_height: Pixels,
    pub weight: FontWeight,
    pub color: Option<Hsla>,
    /// Shift-Enter (or Enter when `enter_newline`) inserts a line break.
    pub multiline: bool,
    pub enter_newline: bool,
    _subs: Vec<Subscription>,
}

impl EventEmitter<InputEvent> for TextInput {}

impl TextInput {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let subs = vec![cx.on_blur(&focus, window, |this: &mut Self, _window, cx| {
            this.marked = None;
            this.selecting = false;
            cx.emit(InputEvent::Blur);
            cx.notify();
        })];
        TextInput {
            focus,
            text: String::new(),
            sel: 0..0,
            reversed: false,
            marked: None,
            layout: None,
            selecting: false,
            placeholder: SharedString::default(),
            font_size: px(14.),
            line_height: px(20.),
            weight: FontWeight::NORMAL,
            color: None,
            multiline: false,
            enter_newline: false,
            _subs: subs,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.text = text.into();
        self.sel = self.text.len()..self.text.len();
        self.reversed = false;
        self.marked = None;
        cx.notify();
    }

    pub fn select_all_text(&mut self, cx: &mut Context<Self>) {
        self.sel = 0..self.text.len();
        self.reversed = false;
        cx.notify();
    }

    pub fn move_to_end(&mut self, cx: &mut Context<Self>) {
        self.sel = self.text.len()..self.text.len();
        cx.notify();
    }

    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    pub fn focus(&self, window: &mut Window) {
        window.focus(&self.focus);
    }

    fn cursor(&self) -> usize {
        if self.reversed {
            self.sel.start
        } else {
            self.sel.end
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = clamp(&self.text, offset);
        self.sel = offset..offset;
        self.reversed = false;
        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = clamp(&self.text, offset);
        if self.reversed {
            self.sel.start = offset;
        } else {
            self.sel.end = offset;
        }
        if self.sel.end < self.sel.start {
            self.reversed = !self.reversed;
            self.sel = self.sel.end..self.sel.start;
        }
        cx.notify();
    }

    fn replace_selection(&mut self, new_text: &str, cx: &mut Context<Self>) {
        let range = self.sel.clone();
        self.text.replace_range(range.clone(), new_text);
        let at = range.start + new_text.len();
        self.sel = at..at;
        self.reversed = false;
        self.marked = None;
        cx.emit(InputEvent::Changed);
        cx.notify();
    }

    // --- actions -----------------------------------------------------------

    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        if self.text.is_empty() {
            cx.emit(InputEvent::BackspaceEmpty);
            return;
        }
        if self.sel.is_empty() {
            let p = prev_grapheme(&self.text, self.cursor());
            self.select_to(p, cx);
        }
        self.replace_selection("", cx);
    }

    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        if self.sel.is_empty() {
            let n = next_grapheme(&self.text, self.cursor());
            self.select_to(n, cx);
        }
        self.replace_selection("", cx);
    }

    fn delete_word_left(&mut self, _: &DeleteWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        if self.sel.is_empty() {
            let p = prev_word(&self.text, self.cursor());
            self.select_to(p, cx);
        }
        self.replace_selection("", cx);
    }

    fn delete_to_line_start(&mut self, _: &DeleteToLineStart, _: &mut Window, cx: &mut Context<Self>) {
        if self.sel.is_empty() {
            let start = self.row_range().start;
            self.select_to(start, cx);
        }
        self.replace_selection("", cx);
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.sel.is_empty() {
            self.move_to(prev_grapheme(&self.text, self.cursor()), cx);
        } else {
            self.move_to(self.sel.start, cx);
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.sel.is_empty() {
            self.move_to(next_grapheme(&self.text, self.cursor()), cx);
        } else {
            self.move_to(self.sel.end, cx);
        }
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        match self.layout.as_ref().and_then(|l| l.index_above(self.cursor(), None)) {
            Some(ix) => self.move_to(ix, cx),
            None => cx.emit(InputEvent::MoveUp),
        }
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        match self.layout.as_ref().and_then(|l| l.index_below(self.cursor(), None)) {
            Some(ix) => self.move_to(ix, cx),
            None => cx.emit(InputEvent::MoveDown),
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(prev_grapheme(&self.text, self.cursor()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(next_grapheme(&self.text, self.cursor()), cx);
    }

    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        let ix = self
            .layout
            .as_ref()
            .and_then(|l| l.index_above(self.cursor(), None))
            .unwrap_or(0);
        self.select_to(ix, cx);
    }

    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        let ix = self
            .layout
            .as_ref()
            .and_then(|l| l.index_below(self.cursor(), None))
            .unwrap_or(self.text.len());
        self.select_to(ix, cx);
    }

    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(prev_word(&self.text, self.cursor()), cx);
    }

    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(next_word(&self.text, self.cursor()), cx);
    }

    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(prev_word(&self.text, self.cursor()), cx);
    }

    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(next_word(&self.text, self.cursor()), cx);
    }

    fn row_range(&self) -> Range<usize> {
        self.layout
            .as_ref()
            .map(|l| l.row_range(self.cursor()))
            .unwrap_or(0..self.text.len())
    }

    fn line_start(&mut self, _: &LineStart, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.row_range().start, cx);
    }

    fn line_end(&mut self, _: &LineEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.row_range().end, cx);
    }

    fn select_line_start(&mut self, _: &SelectLineStart, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.row_range().start, cx);
    }

    fn select_line_end(&mut self, _: &SelectLineEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.row_range().end, cx);
    }

    fn doc_start(&mut self, _: &DocStart, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn doc_end(&mut self, _: &DocEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.text.len(), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.select_all_text(cx);
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.sel.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.text[self.sel.clone()].to_string()));
        }
    }

    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if !self.sel.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.text[self.sel.clone()].to_string()));
            self.replace_selection("", cx);
        }
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let text = if self.multiline {
                text.replace("\r\n", "\n")
            } else {
                text.replace("\r\n", " ").replace('\n', " ")
            };
            self.replace_selection(&text, cx);
        }
    }

    fn enter(&mut self, _: &Enter, _: &mut Window, cx: &mut Context<Self>) {
        if self.multiline && self.enter_newline {
            self.replace_selection("\n", cx);
        } else {
            cx.emit(InputEvent::Submit);
        }
    }

    fn shift_enter(&mut self, _: &ShiftEnter, _: &mut Window, cx: &mut Context<Self>) {
        if self.multiline {
            self.replace_selection("\n", cx);
        } else {
            cx.emit(InputEvent::Submit);
        }
    }

    fn escape(&mut self, _: &Escape, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Cancel);
    }

    // --- mouse ---------------------------------------------------------------

    fn index_for_mouse(&self, event_pos: gpui::Point<Pixels>) -> usize {
        self.layout
            .as_ref()
            .map(|l| l.index_for_point(event_pos))
            .unwrap_or(self.text.len())
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        cx.stop_propagation();
        let ix = self.index_for_mouse(event.position);
        if event.click_count >= 3 {
            self.select_all_text(cx);
            return;
        }
        if event.click_count == 2 {
            let r = word_range_at(&self.text, ix);
            self.sel = r;
            self.reversed = false;
            cx.notify();
            return;
        }
        self.selecting = true;
        if event.modifiers.shift {
            self.select_to(ix, cx);
        } else {
            self.move_to(ix, cx);
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.selecting {
            if event.pressed_button != Some(MouseButton::Left) {
                self.selecting = false;
                return;
            }
            let ix = self.index_for_mouse(event.position);
            self.select_to(ix, cx);
        }
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = range_from_utf16(&self.text, &range_utf16);
        actual_range.replace(range_to_utf16(&self.text, &range));
        Some(self.text[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: range_to_utf16(&self.text, &self.sel),
            reversed: self.reversed,
        })
    }

    fn marked_text_range(&self, _window: &mut Window, _cx: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|r| range_to_utf16(&self.text, r))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|r| range_from_utf16(&self.text, r))
            .or(self.marked.clone())
            .unwrap_or(self.sel.clone());
        let new_text = if self.multiline {
            new_text.to_string()
        } else {
            new_text.replace('\n', " ")
        };
        self.text.replace_range(range.clone(), &new_text);
        let at = range.start + new_text.len();
        self.sel = at..at;
        self.reversed = false;
        self.marked = None;
        cx.emit(InputEvent::Changed);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|r| range_from_utf16(&self.text, r))
            .or(self.marked.clone())
            .unwrap_or(self.sel.clone());
        self.text.replace_range(range.clone(), new_text);
        self.marked = if new_text.is_empty() {
            None
        } else {
            Some(range.start..range.start + new_text.len())
        };
        self.sel = new_selected_range_utf16
            .as_ref()
            .map(|r| range_from_utf16(new_text, r))
            .map(|r| range.start + r.start..range.start + r.end)
            .unwrap_or_else(|| {
                let at = range.start + new_text.len();
                at..at
            });
        self.reversed = false;
        cx.emit(InputEvent::Changed);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.layout.as_ref()?;
        let range = range_from_utf16(&self.text, &range_utf16);
        Some(layout.caret_bounds(range.start))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let layout = self.layout.as_ref()?;
        Some(offset_to_utf16(&self.text, layout.index_for_point(point)))
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focused = self.focus.is_focused(window);
        let color = self.color.unwrap_or(t.text);
        let mut f = font(ui_font());
        f.weight = self.weight;
        let run = TextRun {
            len: self.text.len(),
            font: f.clone(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let placeholder_run = TextRun {
            len: 0,
            font: f,
            color: t.placeholder,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let entity: Entity<Self> = cx.entity();
        let entity2 = entity.clone();
        let focus = self.focus.clone();
        let view = TextView::new(self.text.clone(), vec![run], self.font_size, self.line_height)
            .selection(focused.then(|| self.sel.clone()), t.selection)
            .cursor(
                (focused && self.sel.is_empty()).then(|| self.cursor()),
                t.text,
            )
            .marked(self.marked.clone())
            .placeholder(self.placeholder.clone(), placeholder_run)
            .on_layout(move |layout, _, cx| {
                entity.update(cx, |this, _| this.layout = Some(layout));
            })
            .input(move |bounds, window, cx| {
                window.handle_input(&focus, ElementInputHandler::new(bounds, entity2), cx);
            });

        div()
            .id(("text-input", cx.entity_id()))
            .key_context("TextInput")
            .track_focus(&self.focus)
            .w_full()
            .cursor_text()
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::delete_word_left))
            .on_action(cx.listener(Self::delete_to_line_start))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::line_start))
            .on_action(cx.listener(Self::line_end))
            .on_action(cx.listener(Self::select_line_start))
            .on_action(cx.listener(Self::select_line_end))
            .on_action(cx.listener(Self::doc_start))
            .on_action(cx.listener(Self::doc_end))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::enter))
            .on_action(cx.listener(Self::shift_enter))
            .on_action(cx.listener(Self::escape))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .child(view)
    }
}
