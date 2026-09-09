//! Colours and metrics for the app chrome (PRD §7.2).
//!
//! The launcher "respects the system color scheme and follows the same GPUI theme as the app
//! chrome, so it reads as part of the desktop rather than as a splash screen."

use gpui::{Hsla, Rgba, rgb, rgba};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    Light,
    Dark,
}

impl Appearance {
    /// Follow the desktop's colour scheme.
    ///
    /// The portal's `org.freedesktop.appearance color-scheme` setting is the authoritative source,
    /// but reading it needs an async D-Bus round trip before the first window can be drawn. These
    /// environment hints cover the common cases synchronously; the runtime refreshes from the
    /// portal once the event loop is up.
    pub fn detect() -> Self {
        if let Ok(scheme) = std::env::var("HTMLAPP_COLOR_SCHEME") {
            return if scheme.eq_ignore_ascii_case("light") {
                Appearance::Light
            } else {
                Appearance::Dark
            };
        }
        let gtk_dark = std::env::var("GTK_THEME")
            .map(|t| t.to_ascii_lowercase().contains("dark"))
            .unwrap_or(false);
        if gtk_dark { Appearance::Dark } else { Appearance::Light }
    }
}

/// One resolved colour scheme.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub appearance: Appearance,
    pub background: Rgba,
    pub surface: Rgba,
    pub surface_hover: Rgba,
    pub border: Rgba,
    pub text: Rgba,
    pub text_muted: Rgba,
    pub accent: Rgba,
    pub accent_text: Rgba,
    pub danger: Rgba,
    pub warning: Rgba,
    pub success: Rgba,
}

impl Theme {
    pub fn new(appearance: Appearance) -> Self {
        match appearance {
            Appearance::Dark => Self {
                appearance,
                background: rgb(0x14161a),
                surface: rgb(0x1c1f25),
                surface_hover: rgb(0x252932),
                border: rgb(0x2f343d),
                text: rgb(0xe6e8ec),
                text_muted: rgb(0x9aa1ad),
                accent: rgb(0x4c8dff),
                accent_text: rgb(0xffffff),
                danger: rgb(0xff6b6b),
                warning: rgb(0xffb454),
                success: rgb(0x5ecc8a),
            },
            Appearance::Light => Self {
                appearance,
                background: rgb(0xfbfbfc),
                surface: rgb(0xffffff),
                surface_hover: rgb(0xf1f3f6),
                border: rgb(0xdfe3e9),
                text: rgb(0x1a1d22),
                text_muted: rgb(0x646c78),
                accent: rgb(0x2563eb),
                accent_text: rgb(0xffffff),
                danger: rgb(0xd93a3a),
                warning: rgb(0xb26a00),
                success: rgb(0x1f8a4c),
            },
        }
    }

    pub fn detect() -> Self {
        Self::new(Appearance::detect())
    }

    /// A translucent scrim for modal sheets. Modals painting *over* content is the whole point of
    /// §4.2 G2 — though with the native-child engine backend the page itself still paints last.
    pub fn scrim(&self) -> Hsla {
        match self.appearance {
            Appearance::Dark => rgba(0x000000b0).into(),
            Appearance::Light => rgba(0x1a1d2280).into(),
        }
    }

    /// Colour for a risk level in the consent sheet (§11.2 rule 3).
    pub fn risk_color(&self, risk: htmlapp_caps::Risk) -> Rgba {
        use htmlapp_caps::Risk;
        match risk {
            Risk::Low => self.text_muted,
            Risk::Medium => self.warning,
            Risk::High => self.danger,
            Risk::Extreme => self.danger,
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::detect()
    }
}
