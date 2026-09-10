//! The launcher window (docs/building.md).
//!
//! A native GPUI window with no webview in it at all. The launcher calls that "a deliberate side benefit":
//! it exercises the shell, theming, dialog, and drag-and-drop layers independently of the engine,
//! so it ships before the engine is wired up and it keeps working if the engine fails to start.
//!
//! The launch modes is equally load-bearing: launching with no document is a *normal* launch, not a usage
//! error. That is why this window exists rather than a `--help` dump.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    App, ClickEvent, Context, ExternalPaths, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeyBinding, ParentElement, Render, SharedString, StatefulInteractiveElement,
    Styled, Window, actions, div, px, rems,
};
use htmlapp_caps::{ConsentStore, Recents};

use crate::theme::Theme;

actions!(
    launcher,
    [
        /// Open an `.hta` through the desktop's own file chooser.
        OpenFile,
        /// Write a minimal `.hta` and hand it to the user's text editor.
        NewApp,
        /// Open the consent manager.
        ManagePermissions,
        /// Run whichever recent is selected.
        RunSelected,
        /// Drop the selected recent from the list.
        RemoveSelected,
        SelectNext,
        SelectPrevious,
        /// Copy the diagnostics strip as one block.
        CopyDiagnostics,
    ]
);

/// The launcher's keyboard contract.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-o", OpenFile, None),
        KeyBinding::new("ctrl-n", NewApp, None),
        KeyBinding::new("enter", RunSelected, None),
        KeyBinding::new("delete", RemoveSelected, None),
        KeyBinding::new("down", SelectNext, None),
        KeyBinding::new("up", SelectPrevious, None),
    ]);
}

/// A click handler, as stored rather than as taken by a builder.
type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// What the launcher needs the runtime to do for it.
///
/// The launcher never runs a document itself: the process and instance model requires each document to get its own process,
/// so "open" means "spawn a detached child", which is the runtime's business rather than the UI's.
pub trait LauncherDelegate: Send + Sync + 'static {
    /// Spawn a document in its own process. The launcher stays up (docs/architecture.md).
    fn open_document(&self, path: &Path);
    /// Show the portal file chooser and open whatever comes back.
    fn choose_document(&self);
    /// Write a starter `.hta` somewhere the user picks, then hand it to their editor.
    fn create_blank_app(&self);
    /// Open the consent manager window.
    fn manage_permissions(&self);
    /// Put text on the clipboard.
    fn copy_to_clipboard(&self, text: &str);
    /// Reveal a path in the user's file manager.
    fn reveal(&self, path: &Path);
    /// Open a URL in the user's browser — the identity links in the launcher.
    fn open_url(&self, url: &str);
    /// Forget every stored permission decision for a document (docs/security.md, rule 6).
    fn revoke_permissions(&self, path: &Path);
}

/// Where the identity links in the launcher point.
pub const REPOSITORY_URL: &str = "https://github.com/monzeromer-lab/htmlapp";
pub const DOCS_URL: &str = "https://github.com/monzeromer-lab/htmlapp/tree/main/docs";
pub const LICENSE_URL: &str = "https://www.apache.org/licenses/LICENSE-2.0";

/// One line of the diagnostics strip (docs/building.md).
#[derive(Debug, Clone)]
pub struct Diagnostics {
    pub session_type: String,
    pub compositor: Option<String>,
    pub engine: String,
    pub render_path: String,
    pub gpu: Option<String>,
    pub version: String,
}

impl Diagnostics {
    /// "Copyable as a single block, so bug reports arrive with it."
    pub fn as_block(&self) -> String {
        format!(
            "HTML App {}\n\
             session:      {}\n\
             compositor:   {}\n\
             engine:       {}\n\
             render path:  {}\n\
             gpu:          {}",
            self.version,
            self.session_type,
            self.compositor.as_deref().unwrap_or("unknown"),
            self.engine,
            self.render_path,
            self.gpu.as_deref().unwrap_or("unknown"),
        )
    }
}

/// A bundled example document.
#[derive(Debug, Clone)]
pub struct Example {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
}

/// Where the launcher says the bundled examples live, plus the locations a development or user-local
/// install actually puts them.
pub fn example_directories() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("/usr/share/htmlapp/examples")];
    if let Some(data) = dirs::data_dir() {
        dirs.push(data.join("htmlapp").join("examples"));
    }
    // Running from a checkout.
    if let Ok(exe) = std::env::current_exe()
        && let Some(root) = exe
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
    {
        dirs.push(root.join("examples"));
    }
    dirs
}

/// Read the bundled examples, taking each one's name from its own manifest.
pub fn load_examples() -> Vec<Example> {
    let mut examples = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for directory in example_directories() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("hta") {
                continue;
            }
            let Some(file_name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
                continue;
            };
            if !seen.insert(file_name.clone()) {
                continue;
            }
            let document = htmlapp_caps::Document::load(&path).ok();
            let name = document
                .as_ref()
                .map(|d| d.manifest.display_name().to_string())
                .unwrap_or_else(|| file_name.clone());
            let description = document
                .as_ref()
                .and_then(|d| d.manifest.permissions.as_ref())
                .map(|p| {
                    let modules: Vec<&str> = p.granted_modules().into_iter().collect();
                    if modules.is_empty() {
                        "no permissions".to_string()
                    } else {
                        modules.join(", ")
                    }
                })
                .unwrap_or_else(|| "no permissions".to_string());
            examples.push(Example {
                name,
                description,
                path,
            });
        }
    }

    examples.sort_by(|a, b| a.name.cmp(&b.name));
    examples
}

/// The launcher window's state.
pub struct Launcher {
    theme: Theme,
    delegate: Arc<dyn LauncherDelegate>,
    diagnostics: Diagnostics,
    recents: Recents,
    examples: Vec<Example>,
    consent: ConsentStore,
    selected: usize,
    /// Which recent has its context menu open, and where it was summoned.
    context_menu: Option<(usize, gpui::Point<gpui::Pixels>)>,
    focus: FocusHandle,
}

impl Launcher {
    pub fn new(
        delegate: Arc<dyn LauncherDelegate>,
        diagnostics: Diagnostics,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            theme: Theme::detect(),
            delegate,
            diagnostics,
            recents: Recents::load_default().unwrap_or_default(),
            examples: load_examples(),
            consent: ConsentStore::load_default().unwrap_or_default(),
            selected: 0,
            context_menu: None,
            focus: cx.focus_handle(),
        }
    }

    /// Re-read the recents and consent stores — after a document was opened, for instance.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.recents = Recents::load_default().unwrap_or_default();
        self.consent = ConsentStore::load_default().unwrap_or_default();
        cx.notify();
    }

    fn live_recents(&self) -> Vec<&htmlapp_caps::recents::RecentEntry> {
        self.recents.live().collect()
    }

    fn open(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.delegate.open_document(&path);
        cx.notify();
    }

    fn run_selected(&mut self, _: &RunSelected, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(entry) = self.live_recents().get(self.selected) {
            let path = entry.path.clone();
            self.open(path, cx);
        }
    }

    fn remove_selected(&mut self, _: &RemoveSelected, _: &mut Window, cx: &mut Context<Self>) {
        let path = self
            .live_recents()
            .get(self.selected)
            .map(|e| e.path.clone());
        if let Some(path) = path {
            self.recents.remove(&path);
            let _ = self.recents.save_default();
            self.selected = self.selected.saturating_sub(1);
            cx.notify();
        }
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.live_recents().len();
        if count > 0 {
            self.selected = (self.selected + 1).min(count - 1);
            cx.notify();
        }
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.selected = self.selected.saturating_sub(1);
        cx.notify();
    }

    fn open_file(&mut self, _: &OpenFile, _: &mut Window, _: &mut Context<Self>) {
        self.delegate.choose_document();
    }

    fn new_app(&mut self, _: &NewApp, _: &mut Window, _: &mut Context<Self>) {
        self.delegate.create_blank_app();
    }

    fn manage_permissions(&mut self, _: &ManagePermissions, _: &mut Window, _: &mut Context<Self>) {
        self.delegate.manage_permissions();
    }

    fn copy_diagnostics(&mut self, _: &CopyDiagnostics, _: &mut Window, _: &mut Context<Self>) {
        self.delegate
            .copy_to_clipboard(&self.diagnostics.as_block());
    }
}

impl Focusable for Launcher {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// A relative description of a timestamp, for the recents list.
fn relative_time(then: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let elapsed = now.saturating_sub(then);
    match elapsed {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{} min ago", elapsed / 60),
        3600..=86_399 => format!("{}h ago", elapsed / 3600),
        86_400..=2_591_999 => format!("{}d ago", elapsed / 86_400),
        _ => format!("{} weeks ago", elapsed / 604_800),
    }
}

/// Shorten a path for display, collapsing the home directory to `~`.
fn display_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    if let Some(home) = dirs::home_dir()
        && let Ok(relative) = path.strip_prefix(&home)
    {
        return format!("~/{}", relative.display());
    }
    text.into_owned()
}

impl Render for Launcher {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let delegate = Arc::clone(&self.delegate);

        div()
            .id("launcher")
            .track_focus(&self.focus)
            .key_context("Launcher")
            .on_action(cx.listener(Self::open_file))
            .on_action(cx.listener(Self::new_app))
            .on_action(cx.listener(Self::manage_permissions))
            .on_action(cx.listener(Self::run_selected))
            .on_action(cx.listener(Self::remove_selected))
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::copy_diagnostics))
            // The launcher: "The entire window accepts a dragged .hta."
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _window, cx| {
                for path in paths.paths() {
                    if path.extension().and_then(|e| e.to_str()) == Some("hta") {
                        this.delegate.open_document(path);
                    } else {
                        tracing::info!(path = %path.display(), "ignoring a drop that is not a .hta");
                    }
                }
                cx.notify();
            }))
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.background)
            .text_color(theme.text)
            .font_family("sans-serif")
            .text_size(rems(0.875))
            .child(self.render_header(&theme, &delegate, cx))
            .child(
                div()
                    .id("launcher-body")
                    .flex()
                    .flex_1()
                    .flex_col()
                    .overflow_y_scroll()
                    .p_5()
                    .gap_5()
                    .child(self.render_recents(&theme, cx))
                    .child(self.render_examples(&theme, cx)),
            )
            .child(self.render_diagnostics(&theme, cx))
            .children(self.render_context_menu(&theme, cx))
    }
}

impl Launcher {
    /// Identity and the primary action (docs/building.md).
    fn render_header(
        &self,
        theme: &Theme,
        delegate: &Arc<dyn LauncherDelegate>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let open_delegate = Arc::clone(delegate);
        let new_delegate = Arc::clone(delegate);
        let permissions_delegate = Arc::clone(delegate);
        let theme = *theme;

        div()
            .flex()
            .flex_col()
            .gap_4()
            .p_5()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.surface)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        // The icon, as a mark rather than a bitmap so the launcher has no asset
                        // dependency and still renders if the icon theme is missing.
                        div()
                            .w(px(44.0))
                            .h(px(44.0))
                            .rounded(px(10.0))
                            .bg(theme.accent)
                            .text_color(theme.accent_text)
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(rems(1.1))
                            .child("‹›"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(rems(1.25))
                                    .text_color(theme.text)
                                    .child("HTML App"),
                            )
                            .child(div().text_color(theme.text_muted).child(SharedString::from(
                                format!(
                                    "Run a single .hta file as a desktop application · {}",
                                    self.diagnostics.version
                                ),
                            ))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_3()
                    .text_size(rems(0.75))
                    .child(link("repo-link", "Repository", REPOSITORY_URL, theme, cx))
                    .child(link("docs-link", "Documentation", DOCS_URL, theme, cx))
                    .child(link("license-link", "Apache-2.0", LICENSE_URL, theme, cx)),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(primary_button(
                        "open-file",
                        "Open an .hta file…",
                        "Ctrl+O",
                        theme,
                        cx.listener(move |_, _: &ClickEvent, _, _| open_delegate.choose_document()),
                    ))
                    .child(secondary_button(
                        "new-app",
                        "New blank app",
                        "Ctrl+N",
                        theme,
                        cx.listener(move |_, _: &ClickEvent, _, _| new_delegate.create_blank_app()),
                    ))
                    .child(secondary_button(
                        "permissions",
                        "Permissions…",
                        "",
                        theme,
                        cx.listener(move |_, _: &ClickEvent, _, _| {
                            permissions_delegate.manage_permissions()
                        }),
                    )),
            )
    }

    /// The recents list: name, path, when, and what it was granted (docs/building.md).
    fn render_recents(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *theme;
        let recents = self.live_recents();

        if recents.is_empty() {
            return div()
                .flex()
                .flex_col()
                .gap_2()
                .child(section_title("Recent", theme))
                .child(
                    div()
                        .p_4()
                        .rounded(px(8.0))
                        .border_1()
                        .border_color(theme.border)
                        .text_color(theme.text_muted)
                        .child("Nothing yet. Open an .hta file, or drag one onto this window."),
                );
        }

        let mut list = div().flex().flex_col().gap_1();
        for (index, entry) in recents.iter().enumerate() {
            let path = entry.path.clone();
            let reveal_path = entry.path.clone();
            let selected = index == self.selected;
            let delegate = Arc::clone(&self.delegate);
            let reveal_delegate = Arc::clone(&self.delegate);

            list = list.child(
                div()
                    .id(("recent", index))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .p_3()
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(if selected { theme.accent } else { theme.border })
                    .bg(if selected {
                        theme.surface_hover
                    } else {
                        theme.surface
                    })
                    .hover(|style| style.bg(theme.surface_hover))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.selected = index;
                        delegate.open_document(&path);
                        cx.notify();
                    }))
                    .on_mouse_down(
                        gpui::MouseButton::Right,
                        cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                            this.selected = index;
                            this.context_menu = Some((index, event.position));
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().text_color(theme.text).child(SharedString::from(
                                entry.name.clone().unwrap_or_else(|| {
                                    entry
                                        .path
                                        .file_name()
                                        .map(|n| n.to_string_lossy().into_owned())
                                        .unwrap_or_else(|| "Untitled".into())
                                }),
                            )))
                            .child(
                                div()
                                    .text_size(rems(0.75))
                                    .text_color(theme.text_muted)
                                    .child(SharedString::from(display_path(&entry.path))),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_end()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(rems(0.75))
                                    .text_color(theme.text_muted)
                                    .child(SharedString::from(relative_time(entry.opened_at))),
                            )
                            .child(
                                div()
                                    .text_size(rems(0.75))
                                    .text_color(if entry.granted.is_empty() {
                                        theme.text_muted
                                    } else {
                                        theme.warning
                                    })
                                    .child(SharedString::from(entry.permission_summary())),
                            ),
                    )
                    .child(
                        div()
                            .id(("reveal", index))
                            .px_2()
                            .py_1()
                            .rounded(px(6.0))
                            .text_size(rems(0.75))
                            .text_color(theme.text_muted)
                            .hover(|style| style.bg(theme.surface_hover).text_color(theme.text))
                            .cursor_pointer()
                            .on_click(cx.listener(move |_, _: &ClickEvent, _, _| {
                                reveal_delegate.reveal(&reveal_path)
                            }))
                            .child("Reveal"),
                    ),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(section_title("Recent", theme))
            .child(list)
    }

    /// The bundled examples. The launcher: "These are the tutorial; there is no other onboarding."
    fn render_examples(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *theme;

        if self.examples.is_empty() {
            return div()
                .flex()
                .flex_col()
                .gap_2()
                .child(section_title("Examples", theme))
                .child(
                    div()
                        .p_4()
                        .rounded(px(8.0))
                        .border_1()
                        .border_color(theme.border)
                        .text_color(theme.text_muted)
                        .child(SharedString::from(format!(
                            "No examples found. Looked in: {}",
                            example_directories()
                                .iter()
                                .map(|p| p.display().to_string())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ))),
                );
        }

        let mut list = div().flex().flex_col().gap_1();
        for (index, example) in self.examples.iter().enumerate() {
            let path = example.path.clone();
            let delegate = Arc::clone(&self.delegate);
            list = list.child(
                div()
                    .id(("example", index))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .p_3()
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.surface)
                    .hover(|style| style.bg(theme.surface_hover))
                    .cursor_pointer()
                    .on_click(
                        cx.listener(move |_, _: &ClickEvent, _, _| delegate.open_document(&path)),
                    )
                    .child(
                        div()
                            .text_color(theme.text)
                            .child(SharedString::from(example.name.clone())),
                    )
                    .child(
                        div()
                            .text_size(rems(0.75))
                            .text_color(theme.text_muted)
                            .child(SharedString::from(example.description.clone())),
                    ),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(section_title("Examples", theme))
            .child(list)
    }

    /// The recents context menu the launcher asks for.
    fn render_context_menu(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        let (index, position) = self.context_menu?;
        let entry = self.live_recents().get(index)?.path.clone();
        let theme = *theme;

        let reveal_path = entry.clone();
        let revoke_path = entry.clone();
        let remove_path = entry;
        let reveal_delegate = Arc::clone(&self.delegate);
        let revoke_delegate = Arc::clone(&self.delegate);

        let item = |id: &'static str, label: &'static str, danger: bool, handler: ClickHandler| {
            div()
                .id(id)
                .px_3()
                .py_1p5()
                .cursor_pointer()
                .text_color(if danger { theme.danger } else { theme.text })
                .hover(|style| style.bg(theme.surface_hover))
                .on_click(handler)
                .child(label)
        };

        Some(
            // A full-window catcher, so clicking anywhere else dismisses the menu.
            div()
                .id("context-scrim")
                .absolute()
                .left_0()
                .top_0()
                .size_full()
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.context_menu = None;
                    cx.notify();
                }))
                .child(
                    div()
                        .absolute()
                        .left(position.x)
                        .top(position.y)
                        .min_w(px(200.0))
                        .py_1()
                        .rounded(px(8.0))
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.surface)
                        .text_size(rems(0.8125))
                        .child(item(
                            "ctx-reveal",
                            "Reveal in file manager",
                            false,
                            Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                reveal_delegate.reveal(&reveal_path);
                                this.context_menu = None;
                                cx.notify();
                            })),
                        ))
                        .child(item(
                            "ctx-revoke",
                            "Revoke permissions",
                            true,
                            Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                revoke_delegate.revoke_permissions(&revoke_path);
                                this.context_menu = None;
                                // The summaries in the list are now stale.
                                this.consent = ConsentStore::load_default().unwrap_or_default();
                                cx.notify();
                            })),
                        ))
                        .child(item(
                            "ctx-remove",
                            "Remove from recents",
                            false,
                            Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.recents.remove(&remove_path);
                                let _ = this.recents.save_default();
                                this.context_menu = None;
                                this.selected = this.selected.saturating_sub(1);
                                cx.notify();
                            })),
                        )),
                ),
        )
    }

    /// The launcher's diagnostics strip, copyable as one block.
    fn render_diagnostics(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *theme;
        let delegate = Arc::clone(&self.delegate);
        let block = self.diagnostics.as_block();

        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .px_5()
            .py_2()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.surface)
            .text_size(rems(0.75))
            .text_color(theme.text_muted)
            .child(SharedString::from(format!(
                "{} · {} · {} · {}",
                self.diagnostics.session_type,
                self.diagnostics.compositor.as_deref().unwrap_or("unknown"),
                self.diagnostics.engine,
                self.diagnostics.render_path,
            )))
            .child(
                div()
                    .id("copy-diagnostics")
                    .px_2()
                    .py_1()
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(theme.border)
                    .hover(|style| style.bg(theme.surface_hover).text_color(theme.text))
                    .cursor_pointer()
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, _| {
                        delegate.copy_to_clipboard(&block)
                    }))
                    .child("Copy diagnostics"),
            )
    }
}

fn section_title(text: &'static str, theme: Theme) -> impl IntoElement {
    div()
        .text_size(rems(0.75))
        .text_color(theme.text_muted)
        .child(text)
}

fn primary_button(
    id: &'static str,
    label: &'static str,
    hint: &'static str,
    theme: Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let mut button = div()
        .id(id)
        .flex()
        .items_center()
        .gap_2()
        .px_4()
        .py_2()
        .rounded(px(8.0))
        .bg(theme.accent)
        .text_color(theme.accent_text)
        .cursor_pointer()
        .hover(|style| style.opacity(0.9))
        .on_click(on_click)
        .child(label);
    if !hint.is_empty() {
        button = button.child(div().text_size(rems(0.7)).opacity(0.75).child(hint));
    }
    button
}

fn secondary_button(
    id: &'static str,
    label: &'static str,
    hint: &'static str,
    theme: Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let mut button = div()
        .id(id)
        .flex()
        .items_center()
        .gap_2()
        .px_4()
        .py_2()
        .rounded(px(8.0))
        .border_1()
        .border_color(theme.border)
        .bg(theme.surface)
        .text_color(theme.text)
        .cursor_pointer()
        .hover(|style| style.bg(theme.surface_hover))
        .on_click(on_click)
        .child(label);
    if !hint.is_empty() {
        button = button.child(
            div()
                .text_size(rems(0.7))
                .text_color(theme.text_muted)
                .child(hint),
        );
    }
    button
}

/// One of the identity links in the launcher.
fn link(
    id: &'static str,
    label: &'static str,
    url: &'static str,
    theme: Theme,
    cx: &mut Context<Launcher>,
) -> impl IntoElement {
    div()
        .id(id)
        .cursor_pointer()
        .text_color(theme.accent)
        .hover(|style| style.text_color(theme.text))
        .on_click(cx.listener(move |this, _: &ClickEvent, _, _| this.delegate.open_url(url)))
        .child(label)
}
