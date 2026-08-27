//! Headless terminal state and its canonical ANSI presenter.
//!
//! A [`Screen`] owns one terminal parser for its entire lifetime. PTY reads
//! are chunks of one VT byte stream, not independent documents; keeping the
//! parser beside the terminal is therefore part of the state model.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, MutexGuard};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::Point;
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Processor, Rgb};

const MIN_COLUMNS: usize = 2;
const MIN_SCREEN_LINES: usize = 1;

/// Maximum title length retained in screen state and emitted by the presenter.
pub const MAX_TITLE_CHARS: usize = 256;

#[derive(Default)]
struct PendingEvents {
    pty_writes: VecDeque<String>,
    title: Option<String>,
}

#[derive(Clone, Default)]
struct EventSink {
    pending: Arc<Mutex<PendingEvents>>,
}

impl EventSink {
    fn lock(&self) -> MutexGuard<'_, PendingEvents> {
        self.pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn take_pty_writes(&self) -> Vec<String> {
        self.lock().pty_writes.drain(..).collect()
    }

    fn title(&self) -> Option<String> {
        self.lock().title.clone()
    }
}

impl EventListener for EventSink {
    fn send_event(&self, event: Event) {
        let mut pending = self.lock();
        match event {
            // This queue is deliberately separate from screen state. The
            // daemon can route these replies to the PTY without waiting for
            // journalling, snapshot encoding, or a client pipe.
            Event::PtyWrite(text) => pending.pty_writes.push_back(text),
            Event::Title(title) => pending.title = Some(sanitize_title(&title)),
            Event::ResetTitle => pending.title = None,
            Event::MouseCursorDirty
            | Event::ClipboardStore(_, _)
            | Event::ClipboardLoad(_, _)
            | Event::ColorRequest(_, _)
            | Event::TextAreaSizeRequest(_)
            | Event::CursorBlinkingChange
            | Event::Wakeup
            | Event::Bell
            | Event::Exit
            | Event::ChildExit(_) => {}
        }
    }
}

/// A headless terminal emulator with one persistent VT parser.
pub struct Screen {
    term: Term<EventSink>,
    parser: Processor,
    events: EventSink,
}

impl Screen {
    /// Create a screen. Alacritty requires at least two columns for a
    /// full-width character and at least one visible row.
    ///
    /// The emulator keeps modest scrollback (1,000 lines): the daemon
    /// never delivers history — snapshots carry the visible grid only, and
    /// the SQLite journal is the durable transcript. Alacritty's default
    /// 10,000 lines would cost tens of megabytes of grid per session for
    /// state nothing can ever observe.
    pub fn new(cols: u16, rows: u16) -> Self {
        let size = ScreenSize::new(cols, rows);
        let events = EventSink::default();
        let config = Config {
            scrolling_history: 1_000,
            ..Config::default()
        };
        let term = Term::new(config, &size, events.clone());
        Self {
            term,
            parser: Processor::new(),
            events,
        }
    }

    /// Feed one raw PTY byte chunk into the persistent parser.
    pub fn process(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    /// Feed a chunk and return only the immediate PTY replies generated while
    /// parsing it. This is the low-latency path for DSR/CPR and similar
    /// terminal queries.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<String> {
        self.process(bytes);
        self.take_pty_writes()
    }

    /// Drain PTY replies captured by the event listener.
    pub fn take_pty_writes(&self) -> Vec<String> {
        self.events.take_pty_writes()
    }

    /// Resize the emulator. The terminal library owns primary-grid reflow
    /// and alternate-grid non-reflow behavior; this wrapper only supplies the
    /// authoritative dimensions.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        let size = ScreenSize::new(cols, rows);
        self.term.resize(size);
    }

    /// Return the current dimensions in columns and rows.
    pub fn dimensions(&self) -> (u16, u16) {
        (self.term.columns() as u16, self.term.screen_lines() as u16)
    }

    /// Copy the visible screen and its metadata into owned state.
    pub fn snapshot(&self) -> ScreenSnapshot {
        let (cells, cursor, alternate_screen, bracketed_paste, line_wrap) = {
            let content = self.term.renderable_content();
            let cells = content
                .display_iter
                .map(|indexed| indexed.cell.clone())
                .collect();
            let point = viewport_point(content.display_offset, content.cursor.point);
            let rows = self.term.screen_lines();
            let cols = self.term.columns();
            let cursor_in_viewport = point.is_some_and(|(row, col)| row < rows && col < cols);
            let (cursor_row, cursor_col) = point.unwrap_or((0, 0));
            let cursor_style = self.term.cursor_style();
            let cursor = SnapshotCursor {
                row: cursor_row.min(rows.saturating_sub(1)) as u16,
                col: cursor_col.min(cols.saturating_sub(1)) as u16,
                visible: content.mode.contains(TermMode::SHOW_CURSOR) && cursor_in_viewport,
                shape: snapshot_cursor_shape(cursor_style.shape),
                blinking: cursor_style.blinking,
            };
            (
                cells,
                cursor,
                content.mode.contains(TermMode::ALT_SCREEN),
                content.mode.contains(TermMode::BRACKETED_PASTE),
                content.mode.contains(TermMode::LINE_WRAP),
            )
        };

        let (cols, rows) = self.dimensions();
        ScreenSnapshot {
            cols,
            rows,
            cells,
            cursor,
            alternate_screen,
            bracketed_paste,
            line_wrap,
            title: self.events.title(),
        }
    }

    /// Render the current visible screen as canonical ANSI/VT.
    pub fn render_ansi(&self) -> String {
        render_ansi(&self.snapshot())
    }
}

struct ScreenSize {
    cols: usize,
    rows: usize,
}

impl ScreenSize {
    fn new(cols: u16, rows: u16) -> Self {
        Self {
            cols: usize::from(cols).max(MIN_COLUMNS),
            rows: usize::from(rows).max(MIN_SCREEN_LINES),
        }
    }
}

impl Dimensions for ScreenSize {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

/// A zero-based cursor position and the state needed by a client terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapshotCursor {
    pub row: u16,
    pub col: u16,
    pub visible: bool,
    pub shape: SnapshotCursorShape,
    pub blinking: bool,
}

/// Cursor shapes supported by the canonical ANSI presenter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotCursorShape {
    Block,
    Underline,
    Bar,
}

/// Owned visible terminal state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenSnapshot {
    pub cols: u16,
    pub rows: u16,
    /// Cells are row-major and contain exactly `rows * cols` entries for a
    /// snapshot produced by [`Screen::snapshot`].
    pub cells: Vec<Cell>,
    pub cursor: SnapshotCursor,
    pub alternate_screen: bool,
    pub bracketed_paste: bool,
    pub line_wrap: bool,
    pub title: Option<String>,
}

impl ScreenSnapshot {
    /// Get a cell by zero-based visible row and column.
    pub fn cell(&self, row: u16, col: u16) -> Option<&Cell> {
        let row = usize::from(row);
        let col = usize::from(col);
        if row >= usize::from(self.rows) || col >= usize::from(self.cols) {
            return None;
        }
        self.cells.get(row * usize::from(self.cols) + col)
    }

    /// Render this owned state as canonical ANSI/VT.
    pub fn render_ansi(&self) -> String {
        render_ansi(self)
    }
}

/// Render an owned visible screen to a whitelisted ANSI/VT sequence.
pub fn render_ansi(snapshot: &ScreenSnapshot) -> String {
    let cols = usize::from(snapshot.cols);
    let rows = usize::from(snapshot.rows);
    let mut output = String::with_capacity(snapshot.cells.len() * 3 + 128);

    // Establish the selected buffer and clear it before painting. Resetting
    // first makes the clear operation use the default background.
    output.push_str("\x1b[0m");
    if snapshot.alternate_screen {
        output.push_str("\x1b[?1049h");
    } else {
        output.push_str("\x1b[?1049l");
    }
    output.push_str("\x1b[2J\x1b[H\x1b[0m");

    // Painting uses wrapping even when the captured terminal had it off, so
    // WRAPLINE and the special leading spacer for a final-column wide glyph
    // can be recreated. The original mode is restored at the end.
    output.push_str("\x1b[?7h");
    let mut style = CellStyle::default();
    let mut continuation = None;

    for row in 0..rows {
        let continued_from_previous_row = continuation.take();
        let start_col = continued_from_previous_row.unwrap_or(0);
        if row > 0 && continued_from_previous_row.is_none() {
            cup(&mut output, row, 0);
        }

        let row_wraps = snapshot
            .cell(row as u16, cols.saturating_sub(1) as u16)
            .is_some_and(|cell| cell.flags.contains(Flags::WRAPLINE));
        let mut col = start_col;
        while col < cols {
            let cell = snapshot_cell(snapshot, row, col);

            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                // The preceding WIDE_CHAR writes this cell as part of the
                // same glyph. Never emit the spacer as a second character.
                col += 1;
                continue;
            }

            let is_leading_final_spacer = col + 1 == cols
                && cell.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER)
                && row_wraps
                && row + 1 < rows;
            if is_leading_final_spacer {
                let next = snapshot_cell(snapshot, row + 1, 0);
                if next.flags.contains(Flags::WIDE_CHAR) {
                    set_style(&mut output, &mut style, &cell);
                    // Starting a full-width glyph at the final column makes
                    // Alacritty create the leading spacer and wrap it to the
                    // next row without an extra presenter-specific marker.
                    write_cell_text(&mut output, &next);
                    continuation = Some(2.min(cols));
                    break;
                }
            }

            set_style(&mut output, &mut style, &cell);
            write_cell_text(&mut output, &cell);
            if cell.flags.contains(Flags::WIDE_CHAR) {
                if col + 1 >= cols && row + 1 < rows {
                    continuation = Some(2.min(cols));
                }
                col += 2;
            } else {
                col += 1;
            }
        }

        if row_wraps && continuation.is_none() && row + 1 < rows {
            // The next row's first character triggers the pending wrap and
            // marks this row's final cell with WRAPLINE.
            continuation = Some(0);
        }
    }

    // Leave the target terminal in the exact captured mode and cursor state.
    output.push_str("\x1b[0m");
    if rows > 0 && cols > 0 {
        cup(
            &mut output,
            usize::from(snapshot.cursor.row),
            usize::from(snapshot.cursor.col),
        );
    }
    output.push_str(if snapshot.cursor.visible {
        "\x1b[?25h"
    } else {
        "\x1b[?25l"
    });
    output.push_str("\x1b[");
    output.push_str(&cursor_style_code(snapshot.cursor));
    output.push_str(" q");
    output.push_str(if snapshot.bracketed_paste {
        "\x1b[?2004h"
    } else {
        "\x1b[?2004l"
    });
    output.push_str(if snapshot.line_wrap {
        "\x1b[?7h"
    } else {
        "\x1b[?7l"
    });

    if let Some(title) = snapshot.title.as_deref() {
        output.push_str("\x1b]2;");
        output.push_str(&sanitize_title(title));
        output.push_str("\x1b\\");
    }

    output
}

fn viewport_point(display_offset: usize, point: Point) -> Option<(usize, usize)> {
    let row = point.line.0 + display_offset as i32;
    Some((usize::try_from(row).ok()?, point.column.0))
}

fn snapshot_cursor_shape(shape: CursorShape) -> SnapshotCursorShape {
    match shape {
        CursorShape::Underline => SnapshotCursorShape::Underline,
        CursorShape::Beam => SnapshotCursorShape::Bar,
        CursorShape::Block | CursorShape::HollowBlock | CursorShape::Hidden => {
            SnapshotCursorShape::Block
        }
    }
}

fn cursor_style_code(cursor: SnapshotCursor) -> String {
    let base = match cursor.shape {
        SnapshotCursorShape::Block => 1,
        SnapshotCursorShape::Underline => 3,
        SnapshotCursorShape::Bar => 5,
    };
    (base + u16::from(!cursor.blinking)).to_string()
}

fn snapshot_cell(snapshot: &ScreenSnapshot, row: usize, col: usize) -> Cell {
    snapshot
        .cells
        .get(
            row.saturating_mul(usize::from(snapshot.cols))
                .saturating_add(col),
        )
        .cloned()
        .unwrap_or_default()
}

fn cup(output: &mut String, row: usize, col: usize) {
    let _ = write!(output, "\x1b[{};{}H", row + 1, col + 1);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CellStyle {
    fg: Color,
    bg: Color,
    flags: Flags,
    underline_color: Option<Color>,
}

impl Default for CellStyle {
    fn default() -> Self {
        Self {
            fg: Color::Named(NamedColor::Foreground),
            bg: Color::Named(NamedColor::Background),
            flags: Flags::empty(),
            underline_color: None,
        }
    }
}

impl From<&Cell> for CellStyle {
    fn from(cell: &Cell) -> Self {
        Self {
            fg: cell.fg,
            bg: cell.bg,
            flags: cell.flags
                & (Flags::BOLD
                    | Flags::ITALIC
                    | Flags::UNDERLINE
                    | Flags::DIM
                    | Flags::HIDDEN
                    | Flags::STRIKEOUT
                    | Flags::INVERSE
                    | Flags::DOUBLE_UNDERLINE
                    | Flags::UNDERCURL
                    | Flags::DOTTED_UNDERLINE
                    | Flags::DASHED_UNDERLINE),
            underline_color: cell.underline_color(),
        }
    }
}

fn set_style(output: &mut String, current: &mut CellStyle, cell: &Cell) {
    let next = CellStyle::from(cell);
    if *current == next {
        return;
    }

    output.push_str("\x1b[");
    let mut params = vec!["0".to_string()];
    let flags = next.flags;
    if flags.contains(Flags::BOLD) {
        params.push("1".to_string());
    }
    if flags.contains(Flags::DIM) {
        params.push("2".to_string());
    }
    if flags.contains(Flags::ITALIC) {
        params.push("3".to_string());
    }
    if flags.contains(Flags::DOUBLE_UNDERLINE) {
        params.push("4:2".to_string());
    } else if flags.contains(Flags::UNDERCURL) {
        params.push("4:3".to_string());
    } else if flags.contains(Flags::DOTTED_UNDERLINE) {
        params.push("4:4".to_string());
    } else if flags.contains(Flags::DASHED_UNDERLINE) {
        params.push("4:5".to_string());
    } else if flags.contains(Flags::UNDERLINE) {
        params.push("4".to_string());
    }
    if flags.contains(Flags::INVERSE) {
        params.push("7".to_string());
    }
    if flags.contains(Flags::HIDDEN) {
        params.push("8".to_string());
    }
    if flags.contains(Flags::STRIKEOUT) {
        params.push("9".to_string());
    }
    append_color_params(&mut params, next.fg, true);
    append_color_params(&mut params, next.bg, false);
    if let Some(color) = next.underline_color {
        append_underline_color_params(&mut params, color);
    }
    output.push_str(&params.join(";"));
    output.push('m');
    *current = next;
}

fn append_color_params(params: &mut Vec<String>, color: Color, foreground: bool) {
    let prefix = if foreground { 38 } else { 48 };
    match color {
        Color::Named(named) => params.push(named_color_code(named, foreground).to_string()),
        Color::Indexed(index) => {
            append_color_params_with_prefix(params, Color::Indexed(index), prefix)
        }
        Color::Spec(rgb) => append_color_params_with_prefix(params, Color::Spec(rgb), prefix),
    }
}

fn append_color_params_with_prefix(params: &mut Vec<String>, color: Color, prefix: u16) {
    match color {
        Color::Named(named) => params.push(named_color_code(named, prefix == 38).to_string()),
        Color::Indexed(index) => {
            params.push(prefix.to_string());
            params.push("5".to_string());
            params.push(index.to_string());
        }
        Color::Spec(Rgb { r, g, b }) => {
            params.push(prefix.to_string());
            params.push("2".to_string());
            params.push(r.to_string());
            params.push(g.to_string());
            params.push(b.to_string());
        }
    }
}

fn append_underline_color_params(params: &mut Vec<String>, color: Color) {
    match color {
        Color::Named(named) => {
            if let Some(index) = named_color_index(named) {
                params.push("58".to_string());
                params.push("5".to_string());
                params.push(index.to_string());
            } else {
                params.push("59".to_string());
            }
        }
        Color::Indexed(index) => {
            params.push("58".to_string());
            params.push("5".to_string());
            params.push(index.to_string());
        }
        Color::Spec(Rgb { r, g, b }) => {
            params.push("58".to_string());
            params.push("2".to_string());
            params.push(r.to_string());
            params.push(g.to_string());
            params.push(b.to_string());
        }
    }
}

fn named_color_code(color: NamedColor, foreground: bool) -> u16 {
    let code = match color {
        NamedColor::Black | NamedColor::DimBlack => 30,
        NamedColor::Red | NamedColor::DimRed => 31,
        NamedColor::Green | NamedColor::DimGreen => 32,
        NamedColor::Yellow | NamedColor::DimYellow => 33,
        NamedColor::Blue | NamedColor::DimBlue => 34,
        NamedColor::Magenta | NamedColor::DimMagenta => 35,
        NamedColor::Cyan | NamedColor::DimCyan => 36,
        NamedColor::White | NamedColor::DimWhite => 37,
        NamedColor::BrightBlack => 90,
        NamedColor::BrightRed => 91,
        NamedColor::BrightGreen => 92,
        NamedColor::BrightYellow => 93,
        NamedColor::BrightBlue => 94,
        NamedColor::BrightMagenta => 95,
        NamedColor::BrightCyan => 96,
        NamedColor::BrightWhite => 97,
        NamedColor::Foreground | NamedColor::BrightForeground | NamedColor::DimForeground => 39,
        NamedColor::Background | NamedColor::Cursor => 39,
    };

    if foreground {
        code
    } else if (30..=37).contains(&code) || (90..=97).contains(&code) {
        code + 10
    } else {
        49
    }
}

fn named_color_index(color: NamedColor) -> Option<u8> {
    match color {
        NamedColor::Black => Some(0),
        NamedColor::Red => Some(1),
        NamedColor::Green => Some(2),
        NamedColor::Yellow => Some(3),
        NamedColor::Blue => Some(4),
        NamedColor::Magenta => Some(5),
        NamedColor::Cyan => Some(6),
        NamedColor::White => Some(7),
        NamedColor::BrightBlack => Some(8),
        NamedColor::BrightRed => Some(9),
        NamedColor::BrightGreen => Some(10),
        NamedColor::BrightYellow => Some(11),
        NamedColor::BrightBlue => Some(12),
        NamedColor::BrightMagenta => Some(13),
        NamedColor::BrightCyan => Some(14),
        NamedColor::BrightWhite => Some(15),
        NamedColor::Foreground
        | NamedColor::Background
        | NamedColor::Cursor
        | NamedColor::DimBlack
        | NamedColor::DimRed
        | NamedColor::DimGreen
        | NamedColor::DimYellow
        | NamedColor::DimBlue
        | NamedColor::DimMagenta
        | NamedColor::DimCyan
        | NamedColor::DimWhite
        | NamedColor::BrightForeground
        | NamedColor::DimForeground => None,
    }
}

fn write_cell_text(output: &mut String, cell: &Cell) {
    output.push(safe_char(cell.c));
    if let Some(zerowidth) = cell.zerowidth() {
        for character in zerowidth {
            if !character.is_control() {
                output.push(*character);
            }
        }
    }
}

fn safe_char(character: char) -> char {
    if character.is_control() {
        '\u{fffd}'
    } else {
        character
    }
}

fn sanitize_title(title: &str) -> String {
    title
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_TITLE_CHARS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_same_screen(left: &ScreenSnapshot, right: &ScreenSnapshot) {
        assert_eq!(left, right);
    }

    fn round_trip(source: &Screen) {
        let before = source.snapshot();
        let mut target = Screen::new(before.cols, before.rows);
        target.process(before.render_ansi().as_bytes());
        assert_same_screen(&before, &target.snapshot());
    }

    #[test]
    fn one_persistent_parser_handles_every_split_offset() {
        let bytes = b"\x1b[2;4H\x1b[38;2;12;34;56mwide: \xe7\x95\x8c e\xcc\x81\x1b]2;split-safe\x07\x1b[?2004h";
        let mut whole = Screen::new(20, 4);
        whole.process(bytes);
        let expected = whole.snapshot();

        for split in 0..=bytes.len() {
            let mut screen = Screen::new(20, 4);
            screen.process(&bytes[..split]);
            screen.process(&bytes[split..]);
            assert_same_screen(&expected, &screen.snapshot());
        }

        let mut bytewise = Screen::new(20, 4);
        for byte in bytes {
            bytewise.process(std::slice::from_ref(byte));
        }
        assert_same_screen(&expected, &bytewise.snapshot());
    }

    #[test]
    fn ansi_round_trip_preserves_cells_and_metadata() {
        let mut source = Screen::new(12, 4);
        source.process(
            b"\x1b[?1049h\x1b[?2004h\x1b[5 q\x1b[1;1H\x1b[1;2;3;4:2;7;8;9;31;42mHi\x1b[0m CJK \xe7\x95\x8c e\xcc\x81",
        );
        source.process(b"\x1b[2;1H\x1b[38;5;123;48;2;1;2;3mindexed and rgb");
        source.process(b"\x1b]2;ansi-round-trip\x07");
        round_trip(&source);
    }

    #[test]
    fn wide_glyph_at_final_column_survives_resize_without_panic() {
        let mut screen = Screen::new(4, 2);
        screen.process(b"\x1b[1;4H\xe7\x95\x8c");
        let before = screen.snapshot();
        assert_eq!(before.cols, 4);
        round_trip(&screen);
        screen.resize(3, 2);
        let after = screen.snapshot();
        assert_eq!((after.cols, after.rows), (3, 2));
        let _ = screen.render_ansi();
    }

    #[test]
    fn wide_glyph_round_trip_at_final_column_preserves_every_cell() {
        let mut source = Screen::new(5, 3);
        source.process("abcd界XYZ".as_bytes());
        let rendered = source.render_ansi();

        let mut target = Screen::new(5, 3);
        target.process(rendered.as_bytes());

        let left = source.snapshot();
        let right = target.snapshot();
        for row in 0..left.rows {
            for col in 0..left.cols {
                assert_eq!(
                    left.cell(row, col),
                    right.cell(row, col),
                    "cell mismatch at ({row}, {col})"
                );
            }
        }
    }

    #[test]
    fn combining_marks_stay_on_their_base_cell() {
        let mut screen = Screen::new(8, 2);
        screen.process("e\u{301}\u{308}".as_bytes());
        let snapshot = screen.snapshot();
        let cell = snapshot.cell(0, 0).expect("base cell");
        assert_eq!(cell.c, 'e');
        assert_eq!(cell.zerowidth(), Some(['\u{301}', '\u{308}'].as_slice()));
        round_trip(&screen);
    }

    #[test]
    fn empty_and_blank_screens_round_trip() {
        let empty = Screen::new(6, 3);
        round_trip(&empty);

        let mut blanks = Screen::new(6, 3);
        blanks.process(b"      \r\n      \r\n      ");
        round_trip(&blanks);
    }

    #[test]
    fn alternate_screen_entry_and_exit_preserves_primary_grid() {
        let mut screen = Screen::new(10, 3);
        screen.process(b"primary\x1b[?1049halternate\x1b[?1049l");
        let snapshot = screen.snapshot();
        assert!(!snapshot.alternate_screen);
        assert_eq!(snapshot.cell(0, 0).expect("primary cell").c, 'p');
        assert_eq!(snapshot.cell(0, 1).expect("primary cell").c, 'r');
    }

    #[test]
    fn cursor_at_last_cell_is_zero_based_in_snapshot() {
        let mut screen = Screen::new(4, 2);
        screen.process(b"\x1b[2;4H");
        let cursor = screen.snapshot().cursor;
        assert_eq!((cursor.row, cursor.col), (1, 3));
        round_trip(&screen);
    }

    #[test]
    fn hidden_cursor_and_shape_round_trip() {
        let mut screen = Screen::new(4, 2);
        screen.process(b"\x1b[4 q\x1b[?25l");
        let cursor = screen.snapshot().cursor;
        assert!(!cursor.visible);
        assert_eq!(cursor.shape, SnapshotCursorShape::Underline);
        round_trip(&screen);
    }

    #[test]
    fn dsr_produces_one_one_based_cpr_reply() {
        let mut screen = Screen::new(10, 4);
        screen.process(b"\x1b[3;7H\x1b[6n");
        assert_eq!(screen.take_pty_writes(), vec!["\x1b[3;7R"]);
        assert!(screen.take_pty_writes().is_empty());
    }

    #[test]
    fn title_is_bounded_and_control_free() {
        let mut screen = Screen::new(4, 2);
        screen.process(b"\x1b]2;unsafe\x1b[31m\x07");
        let title = screen.snapshot().title.expect("title");
        assert_eq!(title, "unsafe");

        let mut long = String::from("\x1b]2;");
        long.push_str(&"x".repeat(MAX_TITLE_CHARS + 10));
        long.push('\x07');
        screen.process(long.as_bytes());
        assert_eq!(
            screen.snapshot().title.expect("long title").chars().count(),
            MAX_TITLE_CHARS
        );
    }

    #[test]
    fn wrapped_rows_are_recreated_by_continuous_painting() {
        let mut source = Screen::new(5, 3);
        source.process(b"abcdefghij");
        let snapshot = source.snapshot();
        assert!(snapshot
            .cell(0, 4)
            .expect("wrapped row end")
            .flags
            .contains(Flags::WRAPLINE));
        round_trip(&source);
    }
}
