//! Embedded static assets for the desktop: brand marks rendered by GPUI's
//! SVG rasteriser as alpha masks, so they take the surrounding text colour.

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

const ICONS: &[(&str, &[u8])] = &[
    (
        "icons/superplexr.svg",
        include_bytes!("../assets/icons/superplexr.svg"),
    ),
    (
        "icons/claude.svg",
        include_bytes!("../assets/icons/claude.svg"),
    ),
    (
        "icons/openai.svg",
        include_bytes!("../assets/icons/openai.svg"),
    ),
    (
        "icons/opencode.svg",
        include_bytes!("../assets/icons/opencode.svg"),
    ),
];

pub(crate) struct DesktopAssets;

impl AssetSource for DesktopAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(ICONS
            .iter()
            .find(|(name, _)| *name == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(ICONS
            .iter()
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| SharedString::from(*name))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_brand_mark_is_embedded_and_well_formed() {
        for (name, _) in ICONS {
            let bytes = DesktopAssets
                .load(name)
                .expect("embedded asset")
                .expect("asset present");
            let text = std::str::from_utf8(&bytes).expect("svg is utf-8");
            assert!(text.starts_with("<svg"), "{name} must be an svg");
            assert!(text.contains("viewBox"), "{name} needs a viewBox");
        }
        assert_eq!(DesktopAssets.list("icons/").expect("listing").len(), 4);
        assert!(
            DesktopAssets
                .load("icons/missing.svg")
                .expect("lookup")
                .is_none()
        );
    }
}
