//! Editor content-width preference (S/M/L → 800/1080/1600px).

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WidthMode {
    Narrow,
    #[default]
    Medium,
    Wide,
}

impl WidthMode {
    /// Lowercase wire token stored in the `editorWidth` pref.
    pub fn as_wire(self) -> &'static str {
        match self {
            WidthMode::Narrow => "narrow",
            WidthMode::Medium => "medium",
            WidthMode::Wide => "wide",
        }
    }

    /// Parse a wire token; anything unrecognized ⇒ Medium.
    pub fn from_wire(s: &str) -> WidthMode {
        match s {
            "narrow" => WidthMode::Narrow,
            "wide" => WidthMode::Wide,
            _ => WidthMode::Medium,
        }
    }

    /// The `--content-max-width` pixel value this mode applies.
    pub fn max_width_px(self) -> u32 {
        match self {
            WidthMode::Narrow => 800,
            WidthMode::Medium => 1080,
            WidthMode::Wide => 1600,
        }
    }
}

const EDITOR_WIDTH_KEY: &str = "ogrenotes.editor_width";

fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

/// Cache the mode for the next pre-mount read on the document page.
pub fn cache_editor_width(mode: WidthMode) {
    if let Some(ls) = local_storage() {
        let _ = ls.set_item(EDITOR_WIDTH_KEY, mode.as_wire());
    }
}

/// Read the cached mode, if any.
pub fn read_cached_editor_width() -> Option<WidthMode> {
    let ls = local_storage()?;
    let v = ls.get_item(EDITOR_WIDTH_KEY).ok()??;
    Some(WidthMode::from_wire(&v))
}

/// Cache locally and persist to the server prefs blob.
pub async fn persist_editor_width(
    mode: WidthMode,
) -> Result<(), crate::api::client::ApiClientError> {
    cache_editor_width(mode);
    crate::api::client::api_put(
        "/users/me/prefs",
        &serde_json::json!({ "editorWidth": mode.as_wire() }),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_round_trips() {
        for m in [WidthMode::Narrow, WidthMode::Medium, WidthMode::Wide] {
            assert_eq!(WidthMode::from_wire(m.as_wire()), m);
        }
    }

    #[test]
    fn unknown_wire_defaults_to_medium() {
        assert_eq!(WidthMode::from_wire(""), WidthMode::Medium);
        assert_eq!(WidthMode::from_wire("garbage"), WidthMode::Medium);
        assert_eq!(WidthMode::default(), WidthMode::Medium);
    }

    #[test]
    fn pixel_values_are_fixed() {
        assert_eq!(WidthMode::Narrow.max_width_px(), 800);
        assert_eq!(WidthMode::Medium.max_width_px(), 1080);
        assert_eq!(WidthMode::Wide.max_width_px(), 1600);
    }
}
