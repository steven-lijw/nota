//! A low-level text element: shapes wrapped rich text, paints selection,
//! caret and highlights, and reports its layout back so editors can map
//! between pixels and text offsets.

use gpui::{
    fill, point, px, quad, size, App, AvailableSpace, BorderStyle, Bounds, Element,
    ElementId, GlobalElementId, Hsla, InspectorElementId, IntoElement, LayoutId, Pixels, Point,
    SharedString, Style, TextAlign, TextRun, Window, WrappedLine,
};
use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

/// The result of laying out a [`TextView`], in window coordinates.
#[derive(Clone)]
pub struct TextLayout {
    pub lines: Rc<Vec<WrappedLine>>,
    pub origin: Point<Pixels>,
    pub width: Pixels,
    pub line_height: Pixels,
    pub len: usize,
}

/// Where a text offset sits.
struct Location {
    line: usize,
    row: usize,
    x: Pixels,
    y: Pixels,
}

fn row_starts(line: &WrappedLine) -> Vec<usize> {
    // [0, wrap1, wrap2, ..., len]
    let mut v = Vec::with_capacity(line.wrap_boundaries().len() + 2);
    v.push(0);
    for b in line.wrap_boundaries() {
        if let Some(glyph) = line.runs().get(b.run_ix).and_then(|r| r.glyphs.get(b.glyph_ix)) {
            v.push(glyph.index);
        }
    }
    v.push(line.len());
    v
}

fn prev_char_boundary(text: &str, mut ix: usize) -> usize {
    if ix == 0 {
        return 0;
    }
    ix -= 1;
    while ix > 0 && !text.is_char_boundary(ix) {
        ix -= 1;
    }
    ix
}

impl TextLayout {
    pub fn height(&self) -> Pixels {
        let rows: usize = self.lines.iter().map(|l| l.wrap_boundaries().len() + 1).sum();
        self.line_height * rows.max(1) as f32
    }

    pub fn bounds(&self) -> Bounds<Pixels> {
        Bounds::new(self.origin, size(self.width, self.height()))
    }

    /// (start offset, top y relative to origin) of every line.
    fn line_offsets(&self) -> Vec<(usize, Pixels)> {
        let mut out = Vec::with_capacity(self.lines.len());
        let mut start = 0;
        let mut y = px(0.);
        for line in self.lines.iter() {
            out.push((start, y));
            start += line.len() + 1;
            y += self.line_height * (line.wrap_boundaries().len() + 1) as f32;
        }
        out
    }

    fn locate(&self, index: usize) -> Location {
        let offsets = self.line_offsets();
        let mut li = 0;
        for (i, (start, _)) in offsets.iter().enumerate() {
            if index >= *start {
                li = i;
            }
        }
        let Some(line) = self.lines.get(li) else {
            return Location {
                line: 0,
                row: 0,
                x: px(0.),
                y: px(0.),
            };
        };
        let (start, y) = offsets[li];
        let local = index.saturating_sub(start).min(line.len());
        let rows = row_starts(line);
        let mut row = 0;
        for k in 0..rows.len() - 1 {
            if local >= rows[k] {
                row = k;
            }
        }
        let unwrapped = &line.unwrapped_layout;
        let x = unwrapped.x_for_index(local) - unwrapped.x_for_index(rows[row]);
        Location {
            line: li,
            row,
            x,
            y: y + self.line_height * row as f32,
        }
    }

    /// Offset for a point relative to the text origin.
    pub fn index_for_local_point(&self, p: Point<Pixels>) -> usize {
        if self.lines.is_empty() || p.y < px(0.) {
            return 0;
        }
        let offsets = self.line_offsets();
        let mut li = self.lines.len() - 1;
        let mut found = false;
        for (i, (_, y)) in offsets.iter().enumerate() {
            let h = self.line_height * (self.lines[i].wrap_boundaries().len() + 1) as f32;
            if p.y >= *y && p.y < *y + h {
                li = i;
                found = true;
                break;
            }
        }
        if !found {
            return self.len;
        }
        let line = &self.lines[li];
        let (start, y) = offsets[li];
        let rows = row_starts(line);
        let nrows = rows.len() - 1;
        let row = (((p.y - y) / self.line_height) as usize).min(nrows - 1);
        let unwrapped = &line.unwrapped_layout;
        let x = unwrapped.x_for_index(rows[row]) + p.x.max(px(0.));
        let mut ix = unwrapped.closest_index_for_x(x).clamp(rows[row], rows[row + 1]);
        if row + 1 < nrows && ix >= rows[row + 1] {
            // keep the caret on this visual row rather than the start of the next
            ix = prev_char_boundary(&line.text, rows[row + 1]).max(rows[row]);
        }
        while ix < line.len() && !line.text.is_char_boundary(ix) {
            ix += 1;
        }
        (start + ix).min(self.len)
    }

    pub fn index_for_point(&self, p: Point<Pixels>) -> usize {
        self.index_for_local_point(point(p.x - self.origin.x, p.y - self.origin.y))
    }

    pub fn caret_bounds(&self, index: usize) -> Bounds<Pixels> {
        let loc = self.locate(index);
        Bounds::new(
            point(self.origin.x + loc.x - px(0.5), self.origin.y + loc.y),
            size(px(1.5), self.line_height),
        )
    }

    pub fn is_on_first_row(&self, index: usize) -> bool {
        let loc = self.locate(index);
        loc.line == 0 && loc.row == 0
    }

    pub fn is_on_last_row(&self, index: usize) -> bool {
        let loc = self.locate(index);
        loc.line + 1 >= self.lines.len()
            && loc.row + 1 >= self.lines.last().map_or(1, |l| l.wrap_boundaries().len() + 1)
    }

    /// The x of a caret, relative to the origin.
    pub fn x_of(&self, index: usize) -> Pixels {
        self.locate(index).x
    }

    pub fn index_above(&self, index: usize, goal_x: Option<Pixels>) -> Option<usize> {
        let loc = self.locate(index);
        if loc.y < self.line_height * 0.5 {
            return None;
        }
        Some(self.index_for_local_point(point(
            goal_x.unwrap_or(loc.x),
            loc.y - self.line_height * 0.5,
        )))
    }

    pub fn index_below(&self, index: usize, goal_x: Option<Pixels>) -> Option<usize> {
        let loc = self.locate(index);
        let y = loc.y + self.line_height * 1.5;
        if y > self.height() {
            return None;
        }
        Some(self.index_for_local_point(point(goal_x.unwrap_or(loc.x), y)))
    }

    /// Offset at horizontal position `x` on the first or last visual row.
    pub fn index_at_x_on_edge_row(&self, last: bool, x: Pixels) -> usize {
        let y = if last {
            self.height() - self.line_height * 0.5
        } else {
            self.line_height * 0.5
        };
        self.index_for_local_point(point(x, y))
    }

    /// Start and end offsets of the visual row containing `index`.
    pub fn row_range(&self, index: usize) -> Range<usize> {
        let loc = self.locate(index);
        let Some(line) = self.lines.get(loc.line) else {
            return 0..0;
        };
        let start = self.line_offsets()[loc.line].0;
        let rows = row_starts(line);
        let nrows = rows.len() - 1;
        let row_start = rows[loc.row];
        let row_end = if loc.row + 1 < nrows {
            prev_char_boundary(&line.text, rows[loc.row + 1]).max(row_start)
        } else {
            line.len()
        };
        start + row_start..start + row_end
    }

    /// Rectangles (window coordinates) covering a text range, one per visual row.
    pub fn rects_for_range(&self, range: Range<usize>) -> Vec<Bounds<Pixels>> {
        let mut out = Vec::new();
        if range.is_empty() {
            return out;
        }
        for (li, (start, y)) in self.line_offsets().into_iter().enumerate() {
            let line = &self.lines[li];
            let line_end = start + line.len();
            if range.end < start || range.start > line_end {
                continue;
            }
            let rows = row_starts(line);
            let nrows = rows.len() - 1;
            let unwrapped = &line.unwrapped_layout;
            for row in 0..nrows {
                let rs = start + rows[row];
                let re = start + rows[row + 1];
                let s = range.start.max(rs);
                let e = range.end.min(re);
                let includes_newline = row + 1 == nrows && range.end > line_end && range.start <= line_end;
                if s >= e && !(includes_newline && s <= re) {
                    continue;
                }
                let base = unwrapped.x_for_index(rows[row]);
                let x0 = unwrapped.x_for_index(s.max(rs) - start) - base;
                let mut x1 = unwrapped.x_for_index(e.max(s).min(re) - start) - base;
                if includes_newline {
                    x1 += px(5.);
                }
                if x1 <= x0 {
                    continue;
                }
                out.push(Bounds::new(
                    point(self.origin.x + x0, self.origin.y + y + self.line_height * row as f32),
                    size(x1 - x0, self.line_height),
                ));
            }
        }
        out
    }
}

type LayoutCallback = Box<dyn FnOnce(TextLayout, &mut Window, &mut App)>;
type InputCallback = Box<dyn FnOnce(Bounds<Pixels>, &mut Window, &mut App)>;

pub struct TextView {
    text: SharedString,
    runs: Vec<TextRun>,
    font_size: Pixels,
    line_height: Pixels,
    selection: Option<Range<usize>>,
    cursor: Option<usize>,
    selection_color: Hsla,
    cursor_color: Hsla,
    highlights: Vec<(Range<usize>, Hsla)>,
    marked: Option<Range<usize>>,
    placeholder: Option<(SharedString, TextRun)>,
    on_layout: Option<LayoutCallback>,
    input: Option<InputCallback>,
}

impl TextView {
    pub fn new(text: impl Into<SharedString>, runs: Vec<TextRun>, font_size: Pixels, line_height: Pixels) -> Self {
        TextView {
            text: text.into(),
            runs,
            font_size,
            line_height,
            selection: None,
            cursor: None,
            selection_color: gpui::rgba(0x2383e247).into(),
            cursor_color: gpui::black(),
            highlights: Vec::new(),
            marked: None,
            placeholder: None,
            on_layout: None,
            input: None,
        }
    }

    pub fn selection(mut self, range: Option<Range<usize>>, color: Hsla) -> Self {
        self.selection = range.filter(|r| !r.is_empty());
        self.selection_color = color;
        self
    }

    pub fn cursor(mut self, cursor: Option<usize>, color: Hsla) -> Self {
        self.cursor = cursor;
        self.cursor_color = color;
        self
    }

    pub fn highlights(mut self, highlights: Vec<(Range<usize>, Hsla)>) -> Self {
        self.highlights = highlights;
        self
    }

    pub fn marked(mut self, marked: Option<Range<usize>>) -> Self {
        self.marked = marked;
        self
    }

    pub fn placeholder(mut self, text: impl Into<SharedString>, run: TextRun) -> Self {
        let text: SharedString = text.into();
        let mut run = run;
        run.len = text.len();
        self.placeholder = Some((text, run));
        self
    }

    pub fn on_layout(mut self, f: impl FnOnce(TextLayout, &mut Window, &mut App) + 'static) -> Self {
        self.on_layout = Some(Box::new(f));
        self
    }

    /// Called during paint with the element bounds; use it to register an input handler.
    pub fn input(mut self, f: impl FnOnce(Bounds<Pixels>, &mut Window, &mut App) + 'static) -> Self {
        self.input = Some(Box::new(f));
        self
    }
}

fn shape(
    text: &SharedString,
    runs: &[TextRun],
    font_size: Pixels,
    wrap_width: Option<Pixels>,
    window: &mut Window,
) -> Vec<WrappedLine> {
    // Runs must cover the text exactly; repair if a caller got it wrong.
    let total: usize = runs.iter().map(|r| r.len).sum();
    let fixed;
    let runs = if total != text.len() {
        let mut r = runs.first().cloned().unwrap_or_else(|| TextRun {
            len: 0,
            font: window.text_style().font(),
            color: window.text_style().color,
            background_color: None,
            underline: None,
            strikethrough: None,
        });
        r.len = text.len();
        fixed = vec![r];
        &fixed[..]
    } else {
        runs
    };
    window
        .text_system()
        .shape_text(text.clone(), font_size, runs, wrap_width, None)
        .map(|lines| lines.into_iter().collect())
        .unwrap_or_default()
}

type LayoutCache = Rc<RefCell<Option<(Option<Pixels>, Vec<WrappedLine>, gpui::Size<Pixels>)>>>;

impl IntoElement for TextView {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextView {
    type RequestLayoutState = LayoutCache;
    type PrepaintState = Vec<WrappedLine>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        _cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let cache: LayoutCache = Rc::new(RefCell::new(None));
        let style = Style::default();
        let text = self.text.clone();
        let runs = self.runs.clone();
        let font_size = self.font_size;
        let line_height = self.line_height;
        let layout_id = window.request_measured_layout(style, {
            let cache = cache.clone();
            move |known, available, window, _cx| {
                let width = known.width.or(match available.width {
                    AvailableSpace::Definite(w) => Some(w),
                    _ => None,
                });
                // Intrinsic-size probes without a width: reuse the last real layout,
                // like GPUI's own text element does.
                if width.is_none() {
                    if let Some((_, _, size)) = cache.borrow().as_ref() {
                        return *size;
                    }
                }
                let lines = shape(&text, &runs, font_size, width, window);
                let rows: usize = lines.iter().map(|l| l.wrap_boundaries().len() + 1).sum();
                let content_w = lines
                    .iter()
                    .map(|l| l.width())
                    .fold(px(0.), |a, b| if b > a { b } else { a })
                    .ceil();
                let w = known.width.unwrap_or(content_w);
                let size = size(w, line_height * rows.max(1) as f32);
                *cache.borrow_mut() = Some((width, lines, size));
                size
            }
        });
        (layout_id, cache)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        cache: &mut Self::RequestLayoutState,
        window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        if let Some((Some(w), lines, _)) = cache.borrow_mut().take() {
            if (w - bounds.size.width).abs() < px(0.5) {
                return lines;
            }
        }
        shape(
            &self.text,
            &self.runs,
            self.font_size,
            Some(bounds.size.width),
            window,
        )
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _cache: &mut Self::RequestLayoutState,
        lines: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let layout = TextLayout {
            lines: Rc::new(std::mem::take(lines)),
            origin: bounds.origin,
            width: bounds.size.width,
            line_height: self.line_height,
            len: self.text.len(),
        };

        for (range, color) in &self.highlights {
            for rect in layout.rects_for_range(range.clone()) {
                let rect = Bounds::new(
                    point(rect.origin.x - px(2.), rect.origin.y + px(2.)),
                    size(rect.size.width + px(4.), rect.size.height - px(4.)),
                );
                window.paint_quad(quad(
                    rect,
                    px(4.),
                    *color,
                    px(0.),
                    gpui::transparent_black(),
                    BorderStyle::default(),
                ));
            }
        }

        if let Some(sel) = &self.selection {
            for rect in layout.rects_for_range(sel.clone()) {
                window.paint_quad(fill(rect, self.selection_color));
            }
        }

        let mut y = bounds.origin.y;
        for line in layout.lines.iter() {
            let _ = line.paint(
                point(bounds.origin.x, y),
                self.line_height,
                TextAlign::Left,
                None,
                window,
                cx,
            );
            y += self.line_height * (line.wrap_boundaries().len() + 1) as f32;
        }

        if self.text.is_empty() {
            if let Some((text, run)) = &self.placeholder {
                let lines = shape(
                    text,
                    std::slice::from_ref(run),
                    self.font_size,
                    Some(bounds.size.width.max(px(40.))),
                    window,
                );
                if let Some(line) = lines.first() {
                    let _ = line.paint(
                        bounds.origin,
                        self.line_height,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                }
            }
        }

        if let Some(marked) = &self.marked {
            for rect in layout.rects_for_range(marked.clone()) {
                window.paint_quad(fill(
                    Bounds::new(
                        point(rect.origin.x, rect.origin.y + rect.size.height - px(2.)),
                        size(rect.size.width, px(1.)),
                    ),
                    self.cursor_color,
                ));
            }
        }

        if let Some(cursor) = self.cursor {
            window.paint_quad(fill(layout.caret_bounds(cursor), self.cursor_color));
        }

        if let Some(f) = self.input.take() {
            f(bounds, window, cx);
        }
        if let Some(f) = self.on_layout.take() {
            f(layout, window, cx);
        }
    }
}
