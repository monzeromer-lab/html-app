//! The GPUI chrome: launcher, titlebar, consent sheet, menus, and dialogs (PRD §7.2, §11.2).

#![forbid(unsafe_code)]

pub mod consent;
pub mod launcher;
pub mod permissions;
pub mod theme;
pub mod webview;

pub use consent::{ConsentChoice, ConsentRequest, ConsentSheet};
pub use launcher::{Diagnostics, Launcher, LauncherDelegate};
pub use permissions::{PermissionsDelegate, PermissionsManager};
pub use theme::{Appearance, Theme};
pub use webview::{WebView, WebViewElement};
