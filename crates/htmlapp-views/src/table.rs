//! The `table` native view (PRD §10).
//!
//! Why this cannot be HTML: "millions of rows without DOM". A `<table>` with a million rows costs a
//! million elements; this keeps one row of elements per visible line and indexes into a flat store,
//! so memory is proportional to the viewport rather than the dataset.

use gpui::{AnyElement, IntoElement, ParentElement, SharedString, Styled, div, px, rems, rgb, rgba};
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
    /// Index of the first row to draw. The page scrolls the placeholder; the host scrolls this.
    scroll_top: usize,
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
            scroll_top: 0,
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
        self.scroll_top = 0;
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
        self.scroll_top = 0;
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
        if let Some(top) = props.get("scrollTop").and_then(|s| s.as_u64()) {
            self.scroll_top = (top as usize).min(self.rows.len());
        }
    }

    fn resize(&mut self, _width: f32, _height: f32) {
        // Row height is fixed and columns are laid out by flex, so nothing to recompute.
    }
}

impl TableView {
    /// Build the element for this table.
    ///
    /// A free method rather than a `Render` impl so the host can draw the view straight out of its
    /// own state, without the view having to be a GPUI entity with a `Context` of its own.
    ///
    /// `visible_height` bounds how many rows are built. That is the virtualization: cost is
    /// proportional to the viewport, not to the dataset, which is the whole reason this is not a
    /// `<table>` (§10).
    pub fn element(&self, visible_height: f32) -> AnyElement {
        let capacity = ((visible_height / ROW_HEIGHT).ceil() as usize + 1).min(self.rows.len());
        let first = self.scroll_top.min(self.rows.len().saturating_sub(capacity));

        let header = div()
            .flex()
            .h(px(ROW_HEIGHT))
            .border_b_1()
            .border_color(rgba(0xffffff20))
            .children(self.columns.iter().map(|column| {
                let cell = div()
                    .px_2()
                    .overflow_hidden()
                    .text_color(rgba(0xffffff99))
                    .child(SharedString::from(column.name.clone()));
                match column.width {
                    Some(width) => cell.w(px(width)),
                    None => cell.flex_1(),
                }
            }));

        let rows = self.rows[first..(first + capacity).min(self.rows.len())]
            .iter()
            .enumerate()
            .map(|(offset, row)| {
                let index = first + offset;
                let mut line = div().flex().h(px(ROW_HEIGHT));
                if self.selected == Some(index) {
                    line = line.bg(rgba(0x4c8dff33));
                }
                line.children(self.columns.iter().enumerate().map(|(column_index, column)| {
                    let cell = div()
                        .px_2()
                        .overflow_hidden()
                        .child(row.get(column_index).cloned().unwrap_or_default());
                    match column.width {
                        Some(width) => cell.w(px(width)),
                        None => cell.flex_1(),
                    }
                }))
            })
            .collect::<Vec<_>>();

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x14161a))
            .text_color(rgb(0xdcdfe4))
            .text_size(rems(0.8125))
            .font_family("monospace")
            .child(header)
            .children(rows)
            .into_any_element()
    }
}
