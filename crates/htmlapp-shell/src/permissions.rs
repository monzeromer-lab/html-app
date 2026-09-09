//! The consent manager window (PRD §11.2 rule 6).
//!
//! "`htmlapp permissions` lists every consented file and what it holds. `htmlapp permissions revoke
//! <file>` clears it. A native settings window does the same."
//!
//! Revocation is immediate and unconfirmed on purpose. Taking a permission *away* is the safe
//! direction, and putting a confirmation step in front of it would train people to click through
//! the dialogs that actually matter.

use std::sync::Arc;

use gpui::{
    App, ClickEvent, Context, FocusHandle, Focusable, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, div, px, rems,
};
use htmlapp_caps::{ConsentRecord, ConsentStore};

use crate::theme::Theme;

/// What the manager needs the runtime to do.
pub trait PermissionsDelegate: Send + Sync + 'static {
    fn reveal(&self, path: &std::path::Path);
}

pub struct PermissionsManager {
    theme: Theme,
    store: ConsentStore,
    delegate: Arc<dyn PermissionsDelegate>,
    focus: FocusHandle,
    status: Option<String>,
}

impl PermissionsManager {
    pub fn new(delegate: Arc<dyn PermissionsDelegate>, cx: &mut Context<Self>) -> Self {
        Self {
            theme: Theme::detect(),
            store: ConsentStore::load_default().unwrap_or_default(),
            delegate,
            focus: cx.focus_handle(),
            status: None,
        }
    }

    fn revoke(&mut self, hash: String, cx: &mut Context<Self>) {
        let removed = self.store.revoke_hash(&hash);
        match self.store.save_default() {
            Ok(()) => {
                self.status = Some(format!("Revoked {removed} decision(s)."));
            }
            Err(error) => {
                // Reload rather than leave the UI showing a change that never reached disk.
                self.status = Some(format!("Could not save: {error}"));
                self.store = ConsentStore::load_default().unwrap_or_default();
            }
        }
        cx.notify();
    }

    fn revoke_all(&mut self, cx: &mut Context<Self>) {
        let removed = self.store.revoke_all();
        self.status = match self.store.save_default() {
            Ok(()) => Some(format!("Revoked {removed} decision(s).")),
            Err(error) => {
                self.store = ConsentStore::load_default().unwrap_or_default();
                Some(format!("Could not save: {error}"))
            }
        };
        cx.notify();
    }
}

impl Focusable for PermissionsManager {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for PermissionsManager {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let records: Vec<ConsentRecord> = self.store.records.clone();

        let mut list = div().flex().flex_col().gap_2();
        if records.is_empty() {
            list = list.child(
                div()
                    .p_4()
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(theme.border)
                    .text_color(theme.text_muted)
                    .child("No document has been granted anything yet."),
            );
        }

        for (index, record) in records.iter().enumerate() {
            let modules: Vec<&str> = record.permissions.granted_modules().into_iter().collect();
            let hash = record.hash.clone();
            let reveal_path = record.path.clone();
            let delegate = Arc::clone(&self.delegate);

            let mut row = div()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .p_3()
                .rounded(px(8.0))
                .border_1()
                .border_color(theme.border)
                .bg(theme.surface)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_color(if record.granted { theme.text } else { theme.text_muted })
                                .child(SharedString::from(
                                    record.name.clone().unwrap_or_else(|| "(unnamed)".into()),
                                )),
                        )
                        .child(
                            div()
                                .text_size(rems(0.75))
                                .text_color(theme.text_muted)
                                .child(SharedString::from(
                                    record
                                        .path
                                        .as_ref()
                                        .map(|p| p.display().to_string())
                                        .unwrap_or_else(|| "(embedded document)".into()),
                                )),
                        )
                        .child(
                            div()
                                .text_size(rems(0.7))
                                .text_color(theme.text_muted)
                                // The hash is what the grant is actually pinned to, so it is worth
                                // showing: two files with the same name are not the same grant.
                                .child(SharedString::from(format!(
                                    "sha256 {}…",
                                    &record.hash[..record.hash.len().min(16)]
                                ))),
                        ),
                )
                .child(
                    div()
                        .text_size(rems(0.75))
                        .text_color(if record.granted { theme.warning } else { theme.text_muted })
                        .child(SharedString::from(if !record.granted {
                            "refused".to_string()
                        } else if modules.is_empty() {
                            "no permissions".to_string()
                        } else {
                            modules.join(", ")
                        })),
                );

            if reveal_path.is_some() {
                row = row.child(
                    div()
                        .id(("reveal", index))
                        .px_2()
                        .py_1()
                        .rounded(px(6.0))
                        .text_size(rems(0.75))
                        .text_color(theme.text_muted)
                        .hover(|s| s.bg(theme.surface_hover).text_color(theme.text))
                        .cursor_pointer()
                        .on_click(cx.listener(move |_, _: &ClickEvent, _, _| {
                            if let Some(path) = &reveal_path {
                                delegate.reveal(path);
                            }
                        }))
                        .child("Reveal"),
                );
            }

            row = row.child(
                div()
                    .id(("revoke", index))
                    .px_3()
                    .py_1()
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(theme.danger)
                    .text_size(rems(0.75))
                    .text_color(theme.danger)
                    .hover(|s| s.bg(theme.danger).text_color(theme.accent_text))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.revoke(hash.clone(), cx)
                    }))
                    .child("Revoke"),
            );

            list = list.child(row);
        }

        let has_records = !records.is_empty();

        div()
            .id("permissions-manager")
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.background)
            .text_color(theme.text)
            .font_family("sans-serif")
            .text_size(rems(0.875))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_5()
                    .bg(theme.surface)
                    .border_b_1()
                    .border_color(theme.border)
                    .child(div().text_size(rems(1.1)).child("Permissions"))
                    .child(
                        div()
                            .text_size(rems(0.75))
                            .text_color(theme.text_muted)
                            .child(SharedString::from(format!(
                                "Grants are pinned to each file's contents. Editing a file asks \
                                 again. Stored in {}",
                                ConsentStore::default_path().display()
                            ))),
                    ),
            )
            .child(
                div()
                    .id("permissions-body")
                    .flex()
                    .flex_1()
                    .flex_col()
                    .p_5()
                    .gap_2()
                    .overflow_y_scroll()
                    .child(list),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .p_4()
                    .border_t_1()
                    .border_color(theme.border)
                    .bg(theme.surface)
                    .child(
                        div()
                            .text_size(rems(0.75))
                            .text_color(theme.text_muted)
                            .child(SharedString::from(
                                self.status.clone().unwrap_or_default(),
                            )),
                    )
                    .child(if has_records {
                        div()
                            .id("revoke-all")
                            .px_3()
                            .py_2()
                            .rounded(px(8.0))
                            .border_1()
                            .border_color(theme.danger)
                            .text_color(theme.danger)
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.danger).text_color(theme.accent_text))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.revoke_all(cx)))
                            .child("Revoke everything")
                    } else {
                        div().id("revoke-all-disabled")
                    }),
            )
    }
}
