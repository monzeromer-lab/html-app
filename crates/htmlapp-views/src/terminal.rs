//! The `terminal` native view (docs/bridge.md).
//!
//! Why this cannot be HTML: "Real PTY, real escape handling, 60fps scrollback." Terminal emulation
//! is not text rendering — it is a state machine over escape sequences with a scrollback grid, and
//! reimplementing it in the DOM produces something that looks like a terminal until the first
//! curses program runs in it. `alacritty_terminal` is the same emulator Alacritty ships.
//!
//! The bytes come from `process.pty` over the bridge, so the page drives the terminal but never
//! parses it.

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column as GridColumn, Line as GridLine, Point};
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor, Processor};
use gpui::{AnyElement, IntoElement, ParentElement, Rgba, SharedString, Styled, div, px, rems, rgb};
use serde_json::Value;

use crate::{NativeView, ViewKind};

/// Terminal dimensions in cells.
///
/// `alacritty_terminal` ships a `TermSize` but only inside its test module, so the trait is
/// implemented here rather than depending on something that is not part of its public surface.
#[derive(Debug, Clone, Copy)]
pub struct TermSize {
    pub columns: usize,
    pub screen_lines: usize,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.screen_lines
    }
    fn screen_lines(&self) -> usize {
        self.screen_lines
    }
    fn columns(&self) -> usize {
        self.columns
    }
}

/// Approximate cell metrics, used to translate a pixel rect into a cell grid.
///
/// These are close enough for a monospace face at the default size; the runtime refines them once
/// The text system has measured the actual font.
const CELL_WIDTH: f32 = 8.4;
const CELL_HEIGHT: f32 = 17.0;

pub struct TerminalView {
    term: Term<VoidListener>,
    parser: Processor,
    size: TermSize,
    font_size: f32,
}

impl Default for TerminalView {
    fn default() -> Self {
        Self::new(80, 24)
    }
}

impl TerminalView {
    pub fn new(columns: usize, screen_lines: usize) -> Self {
        let size = TermSize {
            columns: columns.max(1),
            screen_lines: screen_lines.max(1),
        };
        let config = Config {
            scrolling_history: 10_000,
            ..Default::default()
        };

        Self {
            term: Term::new(config, &size, VoidListener),
            parser: Processor::new(),
            size,
            font_size: 13.0,
        }
    }

    pub fn size(&self) -> TermSize {
        self.size
    }

    /// Feed bytes from the PTY through the escape-sequence parser.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    /// The visible grid as plain text, one string per row. Used by tests and by `call("text")`.
    pub fn visible_text(&self) -> Vec<String> {
        let grid = self.term.grid();
        (0..self.size.screen_lines)
            .map(|row| {
                let line = GridLine(row as i32);
                (0..self.size.columns)
                    .map(|column| grid[line][GridColumn(column)].c)
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    /// Where the cursor is, in cells.
    pub fn cursor(&self) -> Point {
        self.term.grid().cursor.point
    }
}

/// Map an alacritty colour to something GPUI can paint.
///
/// The 256-colour cube and greyscale ramp are computed rather than tabulated, which keeps this
/// short and makes the arithmetic auditable against the xterm spec.
fn to_rgba(color: AnsiColor, is_foreground: bool) -> Rgba {
    match color {
        AnsiColor::Spec(rgb_value) => Rgba {
            r: rgb_value.r as f32 / 255.0,
            g: rgb_value.g as f32 / 255.0,
            b: rgb_value.b as f32 / 255.0,
            a: 1.0,
        },
        AnsiColor::Indexed(index) => indexed_to_rgba(index),
        AnsiColor::Named(named) => named_to_rgba(named, is_foreground),
    }
}

fn named_to_rgba(named: NamedColor, is_foreground: bool) -> Rgba {
    let value = match named {
        NamedColor::Black => 0x1c1f25,
        NamedColor::Red => 0xe06c75,
        NamedColor::Green => 0x98c379,
        NamedColor::Yellow => 0xe5c07b,
        NamedColor::Blue => 0x61afef,
        NamedColor::Magenta => 0xc678dd,
        NamedColor::Cyan => 0x56b6c2,
        NamedColor::White => 0xdcdfe4,
        NamedColor::BrightBlack => 0x5c6370,
        NamedColor::BrightRed => 0xff7b86,
        NamedColor::BrightGreen => 0xb5e890,
        NamedColor::BrightYellow => 0xffd68a,
        NamedColor::BrightBlue => 0x7cc5ff,
        NamedColor::BrightMagenta => 0xdd93f0,
        NamedColor::BrightCyan => 0x6fd3de,
        NamedColor::BrightWhite => 0xffffff,
        NamedColor::Foreground | NamedColor::BrightForeground => 0xdcdfe4,
        NamedColor::Background => 0x14161a,
        NamedColor::Cursor => 0xdcdfe4,
        _ => {
            if is_foreground {
                0xdcdfe4
            } else {
                0x14161a
            }
        }
    };
    rgb(value)
}

fn indexed_to_rgba(index: u8) -> Rgba {
    match index {
        0..=15 => named_to_rgba(
            match index {
                0 => NamedColor::Black,
                1 => NamedColor::Red,
                2 => NamedColor::Green,
                3 => NamedColor::Yellow,
                4 => NamedColor::Blue,
                5 => NamedColor::Magenta,
                6 => NamedColor::Cyan,
                7 => NamedColor::White,
                8 => NamedColor::BrightBlack,
                9 => NamedColor::BrightRed,
                10 => NamedColor::BrightGreen,
                11 => NamedColor::BrightYellow,
                12 => NamedColor::BrightBlue,
                13 => NamedColor::BrightMagenta,
                14 => NamedColor::BrightCyan,
                _ => NamedColor::BrightWhite,
            },
            true,
        ),
        // The 6×6×6 colour cube.
        16..=231 => {
            let index = index - 16;
            let step = |v: u8| if v == 0 { 0u32 } else { 55 + v as u32 * 40 };
            let r = step(index / 36);
            let g = step((index % 36) / 6);
            let b = step(index % 6);
            rgb((r << 16) | (g << 8) | b)
        }
        // The 24-step greyscale ramp.
        _ => {
            let level = 8 + (index - 232) as u32 * 10;
            rgb((level << 16) | (level << 8) | level)
        }
    }
}

impl NativeView for TerminalView {
    fn kind(&self) -> ViewKind {
        ViewKind::Terminal
    }

    fn write(&mut self, data: &str) {
        self.feed(data.as_bytes());
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        match method {
            "text" => Ok(Value::from(self.visible_text().join("\n"))),
            "clear" => {
                // The escape sequence rather than a direct grid reset, so scrollback and the
                // emulator's own state stay consistent with what a shell would expect.
                self.feed(b"\x1b[2J\x1b[H");
                Ok(Value::Null)
            }
            "resize" => {
                let columns = params.get("cols").and_then(|c| c.as_u64()).unwrap_or(80) as usize;
                let rows = params.get("rows").and_then(|r| r.as_u64()).unwrap_or(24) as usize;
                self.size = TermSize {
                    columns: columns.max(1),
                    screen_lines: rows.max(1),
                };
                self.term.resize(self.size);
                Ok(Value::Null)
            }
            "cursor" => {
                let point = self.cursor();
                Ok(serde_json::json!({
                    "line": point.line.0,
                    "column": point.column.0,
                }))
            }
            other => Err(format!("no such terminal method: {other}")),
        }
    }

    fn set(&mut self, props: &Value) {
        if let Some(size) = props.get("fontSize").and_then(|s| s.as_f64()) {
            self.font_size = size as f32;
        }
    }

    /// Translate a pixel rect into a cell grid and resize the emulator.
    ///
    /// The PTY has to be told separately — `process.resize` — because the emulator's idea of its
    /// size and the child process's idea of it are different things, and only the page knows which
    /// PTY belongs to this view.
    fn resize(&mut self, width: f32, height: f32) {
        let columns = ((width / CELL_WIDTH).floor() as usize).max(1);
        let screen_lines = ((height / CELL_HEIGHT).floor() as usize).max(1);

        if columns == self.size.columns && screen_lines == self.size.screen_lines {
            return;
        }
        self.size = TermSize {
            columns,
            screen_lines,
        };
        self.term.resize(self.size);
    }
}

impl TerminalView {
    /// Build the element for this terminal.
    ///
    /// A free method rather than a `Render` impl, for the same reason as [`TableView::element`]:
    /// The host draws the view straight out of its own state.
    ///
    /// Adjacent cells sharing a colour are merged into one span, so an ordinary line of text costs
    /// one element rather than eighty.
    pub fn element(&self) -> AnyElement {
        let grid = self.term.grid();
        let mut rows = Vec::with_capacity(self.size.screen_lines);

        for row in 0..self.size.screen_lines {
            let line = GridLine(row as i32);
            let mut spans: Vec<(String, Rgba)> = Vec::new();
            let mut current = String::new();
            let mut current_color: Option<Rgba> = None;

            for column in 0..self.size.columns {
                let cell = &grid[line][GridColumn(column)];
                let color = to_rgba(cell.fg, true);
                if Some(color) != current_color {
                    if !current.is_empty()
                        && let Some(previous) = current_color
                    {
                        spans.push((std::mem::take(&mut current), previous));
                    }
                    current_color = Some(color);
                }
                current.push(cell.c);
            }
            if !current.trim_end().is_empty()
                && let Some(color) = current_color
            {
                spans.push((current.trim_end().to_string(), color));
            }

            rows.push(
                div()
                    .flex()
                    .h(px(CELL_HEIGHT))
                    .children(spans.into_iter().map(|(text, color)| {
                        div().text_color(color).child(SharedString::from(text))
                    })),
            );
        }

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x14161a))
            .font_family("monospace")
            .text_size(rems(self.font_size / 16.0))
            .children(rows)
            .into_any_element()
    }
}
