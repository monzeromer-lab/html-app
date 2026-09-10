//! Native views (docs/bridge.md).
//!
//! These test the parts that do not need a window: the terminal's escape handling and the table's
//! data model. Rendering is GPUI's job and is exercised by running the app.

use htmlapp_views::{NativeView, TableView, TerminalView, ViewKind};
use serde_json::json;

// --- terminal ---

/// The reason this is not HTML: a real emulator, with real escape handling.
#[test]
fn terminal_handles_escape_sequences() {
    let mut term = TerminalView::new(20, 5);

    term.write("hello");
    assert_eq!(term.visible_text()[0], "hello");

    // Carriage return moves to column 0 without a newline; the next write overwrites.
    term.write("\r\nworld");
    assert_eq!(term.visible_text()[1], "world");

    // Cursor positioning: CUP to row 1, column 1.
    term.write("\x1b[1;1H");
    assert_eq!(term.cursor().line.0, 0);
    assert_eq!(term.cursor().column.0, 0);

    // Erase display, then check it really is gone.
    term.write("\x1b[2J");
    assert!(
        term.visible_text().iter().all(|line| line.is_empty()),
        "ED should have cleared the screen: {:?}",
        term.visible_text()
    );
}

/// SGR colour codes must be consumed as escapes, not printed as text.
#[test]
fn terminal_does_not_print_colour_escapes() {
    let mut term = TerminalView::new(40, 3);
    term.write("\x1b[31mred\x1b[0m plain");

    let line = &term.visible_text()[0];
    assert_eq!(line, "red plain");
    assert!(
        !line.contains('\x1b'),
        "escape leaked into the text: {line:?}"
    );
    assert!(!line.contains("31m"), "SGR parameters leaked: {line:?}");
}

#[test]
fn terminal_wraps_at_the_right_edge() {
    let mut term = TerminalView::new(5, 4);
    term.write("abcdefgh");
    let text = term.visible_text();
    assert_eq!(text[0], "abcde");
    assert_eq!(text[1], "fgh");
}

#[test]
fn terminal_resizes_from_a_pixel_rect() {
    let mut term = TerminalView::new(80, 24);
    // 800×340 logical pixels at roughly 8.4×17 per cell.
    term.resize(800.0, 340.0);

    let size = term.size();
    assert!(
        size.columns > 80 && size.columns < 110,
        "columns: {}",
        size.columns
    );
    assert_eq!(size.screen_lines, 20);

    // A zero-sized rect must not produce a zero-sized grid, which would panic the emulator.
    term.resize(0.0, 0.0);
    assert_eq!(term.size().columns, 1);
    assert_eq!(term.size().screen_lines, 1);
}

#[test]
fn terminal_exposes_its_text_over_the_bridge() {
    let mut term = TerminalView::new(20, 3);
    term.write("visible");
    let text = term.call("text", json!({})).unwrap();
    assert!(text.as_str().unwrap().contains("visible"));

    term.call("clear", json!({})).unwrap();
    let cleared = term.call("text", json!({})).unwrap();
    assert!(cleared.as_str().unwrap().trim().is_empty());

    assert!(term.call("nonsense", json!({})).is_err());
}

// --- table ---

#[test]
fn table_formats_rows_from_objects_and_arrays() {
    let mut table = TableView::new();
    table
        .call("setColumns", json!({ "columns": ["name", "score"] }))
        .unwrap();

    table
        .call(
            "append",
            json!({ "rows": [
                { "name": "ada", "score": 91 },
                { "name": "grace", "score": 88 },
            ]}),
        )
        .unwrap();
    assert_eq!(table.row_count(), 2);

    // Arrays are positional, which is what a SQL result set naturally produces.
    table
        .call("append", json!({ "rows": [["alan", 95]] }))
        .unwrap();
    assert_eq!(table.row_count(), 3);

    assert_eq!(table.call("count", json!({})).unwrap(), json!(3));
}

/// Rows can be streamed in as newline-delimited JSON, so a query does not have to complete first.
#[test]
fn table_accepts_streamed_rows() {
    let mut table = TableView::new();
    table
        .call("setColumns", json!({ "columns": ["a"] }))
        .unwrap();

    table.write("{\"a\":1}\n{\"a\":2}\n\n{\"a\":3}\n");
    assert_eq!(table.row_count(), 3, "blank lines must be skipped");

    // A malformed row is dropped rather than taking the whole view down.
    table.write("not json\n");
    assert_eq!(table.row_count(), 3);
}

#[test]
fn table_columns_can_carry_widths() {
    let mut table = TableView::new();
    table
        .call(
            "setColumns",
            json!({ "columns": [
                { "name": "id", "width": 60 },
                { "name": "message" },
            ]}),
        )
        .unwrap();

    assert_eq!(table.columns()[0].width, Some(60.0));
    assert_eq!(table.columns()[1].width, None);
}

#[test]
fn changing_columns_clears_stale_rows() {
    let mut table = TableView::new();
    table
        .call("setColumns", json!({ "columns": ["a"] }))
        .unwrap();
    table
        .call("append", json!({ "rows": [{ "a": 1 }] }))
        .unwrap();
    assert_eq!(table.row_count(), 1);

    // Rows formatted for the old columns would be meaningless under the new ones.
    table
        .call("setColumns", json!({ "columns": ["b", "c"] }))
        .unwrap();
    assert_eq!(table.row_count(), 0);
}

// --- kinds ---

#[test]
fn view_kinds_report_what_is_actually_implemented() {
    assert_eq!(ViewKind::parse("terminal"), Some(ViewKind::Terminal));
    assert_eq!(ViewKind::parse("table"), Some(ViewKind::Table));
    assert_eq!(ViewKind::parse("nonsense"), None);

    // The roadmap ships terminal and table in M5; editor, video, and canvas3d are M8.
    assert!(ViewKind::Terminal.is_implemented());
    assert!(ViewKind::Table.is_implemented());
    assert!(!ViewKind::Editor.is_implemented());
    assert!(!ViewKind::Video.is_implemented());
}
