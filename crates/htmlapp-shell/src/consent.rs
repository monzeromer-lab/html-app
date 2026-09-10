//! The consent sheet (docs/security.md, rule 3).
//!
//! "On first run of an unknown file, a native GPUI sheet lists exactly what was requested, in plain
//! language, with the source path. The decision is stored keyed on `sha256(file)`. Editing the file
//! invalidates consent and re-prompts, with a diff of what changed in the permission set."
//!
//! Two things about the design are deliberate. The default action is **Run without permissions**,
//! not Allow — the security model, rule 1 makes a powerless document a working document, so the safe choice is
//! also a useful one. And an escalation over a previously-trusted version of the same file is
//! called out explicitly, because the threat model lists "trojan update to a trusted file" as a named threat
//! and a silent re-prompt would look identical to a first run.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    App, ClickEvent, Context, FocusHandle, Focusable, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, div, px, rems,
};
use htmlapp_caps::{PermissionDescription, PermissionDiff, Permissions, Risk};

use crate::theme::Theme;

/// What the user chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsentChoice {
    /// Grant everything requested and remember it against this file's hash.
    Allow,
    /// Run the document as a plain page with no native APIs (docs/security.md, rule 1).
    RunPowerless,
    /// Do not run it at all.
    Cancel,
}

/// Everything the sheet needs to describe the request.
#[derive(Debug, Clone)]
pub struct ConsentRequest {
    pub app_name: String,
    pub source: Option<PathBuf>,
    pub hash: String,
    pub permissions: Permissions,
    /// `Some` when a different version of this same path was decided on before.
    pub diff: Option<PermissionDiff>,
    pub previously_seen: bool,
}

impl ConsentRequest {
    pub fn descriptions(&self) -> Vec<PermissionDescription> {
        self.permissions.describe()
    }

    /// Whether this request adds capability over what was previously trusted.
    pub fn is_escalation(&self) -> bool {
        self.diff
            .as_ref()
            .is_some_and(PermissionDiff::is_escalation)
    }

    /// The highest risk level anywhere in the request, which sets the sheet's tone.
    pub fn peak_risk(&self) -> Option<Risk> {
        self.descriptions().iter().map(|d| d.risk).max()
    }
}

/// The sheet itself.
pub struct ConsentSheet {
    theme: Theme,
    request: ConsentRequest,
    on_choice: Arc<dyn Fn(ConsentChoice) + Send + Sync>,
    focus: FocusHandle,
}

impl ConsentSheet {
    pub fn new(
        request: ConsentRequest,
        on_choice: Arc<dyn Fn(ConsentChoice) + Send + Sync>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            theme: Theme::detect(),
            request,
            on_choice,
            focus: cx.focus_handle(),
        }
    }
}

impl Focusable for ConsentSheet {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ConsentSheet {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let request = self.request.clone();

        let allow = Arc::clone(&self.on_choice);
        let powerless = Arc::clone(&self.on_choice);
        let cancel = Arc::clone(&self.on_choice);

        div()
            .id("consent")
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.background)
            .text_color(theme.text)
            .font_family("sans-serif")
            .text_size(rems(0.875))
            .child(self.render_header(&request, theme))
            .child(
                div()
                    .id("consent-body")
                    .flex()
                    .flex_1()
                    .flex_col()
                    .gap_3()
                    .p_5()
                    .overflow_y_scroll()
                    .children(self.render_diff(&request, theme))
                    .child(self.render_permissions(&request, theme)),
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
                            .id("cancel")
                            .px_4()
                            .py_2()
                            .rounded(px(8.0))
                            .text_color(theme.text_muted)
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.surface_hover).text_color(theme.text))
                            .on_click(cx.listener(move |_, _: &ClickEvent, _, _| {
                                cancel(ConsentChoice::Cancel)
                            }))
                            .child("Don't open"),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            // The safe option is presented first and styled as the default,
                            // because it is also a working option.
                            .child(
                                div()
                                    .id("powerless")
                                    .px_4()
                                    .py_2()
                                    .rounded(px(8.0))
                                    .border_1()
                                    .border_color(theme.accent)
                                    .text_color(theme.text)
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme.surface_hover))
                                    .on_click(cx.listener(move |_, _: &ClickEvent, _, _| {
                                        powerless(ConsentChoice::RunPowerless)
                                    }))
                                    .child("Open without permissions"),
                            )
                            .child(
                                div()
                                    .id("allow")
                                    .px_4()
                                    .py_2()
                                    .rounded(px(8.0))
                                    .bg(if request.peak_risk() >= Some(Risk::High) {
                                        theme.danger
                                    } else {
                                        theme.accent
                                    })
                                    .text_color(theme.accent_text)
                                    .cursor_pointer()
                                    .hover(|s| s.opacity(0.9))
                                    .on_click(cx.listener(move |_, _: &ClickEvent, _, _| {
                                        allow(ConsentChoice::Allow)
                                    }))
                                    .child("Allow and open"),
                            ),
                    ),
            )
    }
}

impl ConsentSheet {
    fn render_header(&self, request: &ConsentRequest, theme: Theme) -> impl IntoElement {
        let source = request
            .source
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "an embedded document".to_string());

        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_5()
            .bg(theme.surface)
            .border_b_1()
            .border_color(theme.border)
            .child(div().text_size(rems(1.1)).child(SharedString::from(format!(
                "{} wants access to this computer",
                request.app_name
            ))))
            // The security model, rule 3 requires the source path: the file's own name is chosen by whoever
            // wrote it, so it is not evidence of anything.
            .child(
                div()
                    .text_size(rems(0.75))
                    .text_color(theme.text_muted)
                    .child(SharedString::from(source)),
            )
            .child(
                div()
                    .text_size(rems(0.7))
                    .text_color(theme.text_muted)
                    .child(SharedString::from(format!(
                        "sha256 {}…",
                        &request.hash[..request.hash.len().min(16)]
                    ))),
            )
    }

    /// The escalation banner, when a previously-trusted file has changed (docs/security.md).
    fn render_diff(&self, request: &ConsentRequest, theme: Theme) -> Option<impl IntoElement> {
        let diff = request.diff.as_ref()?;
        if !request.previously_seen || diff.is_empty() {
            return None;
        }

        let mut lines = Vec::new();
        if !diff.added.is_empty() {
            lines.push(format!("Now also asks for: {}", diff.added.join(", ")));
        }
        if !diff.changed.is_empty() {
            lines.push(format!("Wider access to: {}", diff.changed.join(", ")));
        }
        if !diff.removed.is_empty() {
            lines.push(format!("No longer asks for: {}", diff.removed.join(", ")));
        }

        let escalating = diff.is_escalation();
        Some(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .p_3()
                .rounded(px(8.0))
                .border_1()
                .border_color(if escalating {
                    theme.warning
                } else {
                    theme.border
                })
                .child(
                    div()
                        .text_color(if escalating {
                            theme.warning
                        } else {
                            theme.text
                        })
                        .child("This file has changed since you last allowed it."),
                )
                .children(lines.into_iter().map(|line| {
                    div()
                        .text_size(rems(0.8))
                        .text_color(theme.text_muted)
                        .child(SharedString::from(line))
                })),
        )
    }

    /// The plain-language permission list, most alarming first.
    fn render_permissions(&self, request: &ConsentRequest, theme: Theme) -> impl IntoElement {
        let descriptions = request.descriptions();

        if descriptions.is_empty() {
            return div()
                .p_4()
                .rounded(px(8.0))
                .border_1()
                .border_color(theme.border)
                .text_color(theme.text_muted)
                .child("This document asks for nothing. It will run as an ordinary page.");
        }

        let mut list = div().flex().flex_col().gap_2();
        for description in descriptions {
            let risk_color = theme.risk_color(description.risk);
            list = list.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_3()
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(if description.risk >= Risk::High {
                        risk_color
                    } else {
                        theme.border
                    })
                    .bg(theme.surface)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .text_color(theme.text)
                                    .child(SharedString::from(description.summary.clone())),
                            )
                            .child(div().text_size(rems(0.7)).text_color(risk_color).child(
                                SharedString::from(match description.risk {
                                    Risk::Low => "low risk",
                                    Risk::Medium => "medium risk",
                                    Risk::High => "high risk",
                                    Risk::Extreme => "voids every other protection",
                                }),
                            )),
                    )
                    .child(
                        div()
                            .text_size(rems(0.8))
                            .text_color(theme.text_muted)
                            .child(SharedString::from(description.detail.clone())),
                    ),
            );
        }
        list
    }
}
