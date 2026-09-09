//! The `table` native view (PRD §10).
//!
//! Why this cannot be HTML: "millions of rows without DOM". A `<table>` with a million rows costs a
//! million elements; this keeps one row of elements per visible line and indexes into a flat store,
//! so memory is proportional to the viewport rather than the dataset.

use gpui::{
    Context, IntoElement, ParentElement, Render, SharedString, Styled, Window, div, px, rems,
    uniform_list,
};
use serde_json::Value;

use crate::{NativeView, ViewKind};

/// The height of one row. Fixed, because uniform rows are what make virtualization O(viewport).
const ROW_HEIGHT: f32 = 24.0;

#[derive(Debug, Clone, Default)]
pub struct Column {
    pub name: String,
    /// Width in pixels. `None` shares the remaining space equally.
    pub width: Option<f32>,
}

/// A virtualized table.
pub struct TableView {
    columns: Vec<Column>,
    /// Rows as flat strings: the view never needs the original types, and storing them as text
    /// avoids re-formatting a value every time it scrolls back into sight.
    rows: Vec<Vec<SharedString>>,
    selected: Option<usize>,
}

impl Default for TableView {
    fn default() -> Self {
        Self::new()
    }
}

impl TableView {
    pub fn new() -> Self {
        Self {
            columns: Vec::new(),
            rows: Vec::new(),
            selected: None,
        }
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    pub fn columns(&self) -> &[Column] {
        &self.columns
    }

    /// Replace the columns, clearing any rows that no longer fit them.
    pub fn set_columns(&mut self, columns: Vec<Column>) {
        self.columns = columns;
        self.rows.clear();
        self.selected = None;
    }

    /// Append rows. Values are stringified once, here.
    pub fn append(&mut self, rows: &[Value]) {
        for row in rows {
            self.rows.push(self.format_row(row));
        }
    }

    pub fn clear(&mut self) {
        self.rows.clear();
        self.selected = None;
    }

    /// Turn one JSON row — object or array — into display strings, column by column.
    fn format_row(&self, row: &Value) -> Vec<SharedString> {
        self.columns
            .iter()
            .enumerate()
            .map(|(index, column)| {
                let value = match row {
                    Value::Object(map) => map.get(&column.name),
                    Value::Array(items) => items.get(index),
                    other => Some(other),
                };
                SharedString::from(match value {
                    None | Some(Value::Null) => String::new(),
                    // Strings are shown as-is; anything else gets its JSON form, which is the
                    // least surprising rendering for numbers, booleans, and nested values alike.
                    Some(Value::String(text)) => text.clone(),
                    Some(other) => other.to_string(),
                })
            })
            .collect()
    }
}

impl NativeView for TableView {
    fn kind(&self) -> ViewKind {
        ViewKind::Table
    }

    fn write(&mut self, data: &str) {
        // A newline-delimited JSON stream, so a query can append rows as they arrive.
        for line in data.lines().filter(|l| !l.trim().is_empty()) {
            match serde_json::from_str::<Value>(line) {
                Ok(row) => self.rows.push(self.format_row(&row)),
                Err(error) => tracing::warn!(%error, "table row is not valid JSON"),
            }
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        match method {
            "setColumns" => {
                let columns = params
                    .get("columns")
                    .and_then(|c| c.as_array())
                    .ok_or("columns must be an array")?
                    .iter()
                    .map(|column| match column {
                        Value::String(name) => Column {
                            name: name.clone(),
                            width: None,
                        },
                        other => Column {
                            name: other
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            width: other.get("width").and_then(|w| w.as_f64()).map(|w| w as f32),
                        },
                    })
                    .collect();
                self.set_columns(columns);
                Ok(Value::Null)
            }
            "append" => {
                let rows = params
                    .get("rows")
                    .and_then(|r| r.as_array())
                    .ok_or("rows must be an array")?;
                self.append(rows);
                Ok(Value::Null)
            }
            "clear" => {
                self.clear();
                Ok(Value::Null)
            }
            "count" => Ok(Value::from(self.rows.len())),
            other => Err(format!("no such table method: {other}")),
        }
    }

    fn set(&mut self, props: &Value) {
        if let Some(selected) = props.get("selected").and_then(|s| s.as_u64()) {
            self.selected = Some(selected as usize);
        }
    }

    fn resize(&mut self, _width: f32, _height: f32) {
        // Row height is fixed and columns are laid out by flex, so nothing to recompute.
    }
}

impl Render for TableView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let columns = self.columns.clone();
        let rows = self.rows.clone();
        let selected = self.selected;
        let count = rows.len();

        let header = div()
            .flex()
            .h(px(ROW_HEIGHT))
            .border_b_1()
            .children(columns.iter().map(|column| {
                let mut cell = div()
                    .px_2()
                    .overflow_hidden()
                    .child(SharedString::from(column.name.clone()));
                cell = match column.width {
                    Some(width) => cell.w(px(width)),
                    None => cell.flex_1(),
                };
                cell
            }));

        let body = uniform_list(
            "table-rows",
            count,
            cx.processor(move |_this, range: std::ops::Range<usize>, _window, _cx| {
                // Only the visible slice is ever built — this is the whole point of the view.
                range
                    .map(|index| {
                        let row = &rows[index];
                        let mut line = div().flex().h(px(ROW_HEIGHT));
                        if selected == Some(index) {
                            line = line.bg(gpui::rgba(0x4c8dff26));
                        }
                        line
                            .children(columns.iter().enumerate().map(|(column_index, column)| {
                                let mut cell = div()
                                    .px_2()
                                    .overflow_hidden()
                                    .child(row.get(column_index).cloned().unwrap_or_default());
                                cell = match column.width {
                                    Some(width) => cell.w(px(width)),
                                    None => cell.flex_1(),
                                };
                                cell
                            }))
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .flex_1();

        div()
            .flex()
            .flex_col()
            .size_full()
            .text_size(rems(0.8125))
            .child(header)
            .child(body)
    }
}
