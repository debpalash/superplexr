use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        LazyLock, RwLock,
        atomic::{AtomicU32, Ordering},
    },
};

use gpui::{Rgba, SharedString};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::extension::{
    ExtensionAdapter, ExtensionCapability, ExtensionManifest, ExtensionManifestError,
    ExtensionSourceKind, validate_manifest,
};

const THEME_FILE_VERSION: u16 = 1;
const MAX_THEME_BYTES: u64 = 64 * 1024;
const MAX_NAME_BYTES: usize = 64;
const MAX_FONT_BYTES: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ColorToken {
    Deck,
    Panel,
    Active,
    Hairline,
    Chalk,
    Trace,
    Relay,
    Signal,
    Success,
    Fault,
}

pub(crate) const DECK: ColorToken = ColorToken::Deck;
pub(crate) const PANEL: ColorToken = ColorToken::Panel;
pub(crate) const ACTIVE: ColorToken = ColorToken::Active;
pub(crate) const HAIRLINE: ColorToken = ColorToken::Hairline;
pub(crate) const CHALK: ColorToken = ColorToken::Chalk;
pub(crate) const TRACE: ColorToken = ColorToken::Trace;
pub(crate) const RELAY: ColorToken = ColorToken::Relay;
pub(crate) const SIGNAL: ColorToken = ColorToken::Signal;
pub(crate) const SUCCESS: ColorToken = ColorToken::Success;
pub(crate) const FAULT: ColorToken = ColorToken::Fault;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FontToken {
    Product,
    Terminal,
}

pub(crate) const PRODUCT_FONT: FontToken = FontToken::Product;
pub(crate) const UI_FONT: FontToken = FontToken::Terminal;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "source", content = "value", rename_all = "snake_case")]
pub(crate) enum ThemeSelection {
    BuiltIn(String),
    File(PathBuf),
}

impl Default for ThemeSelection {
    fn default() -> Self {
        Self::BuiltIn("graphite".to_owned())
    }
}

impl ThemeSelection {
    #[cfg(not(test))]
    pub(crate) fn from_argument(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "graphite" | "paper" => Self::BuiltIn(value.trim().to_ascii_lowercase()),
            _ => {
                let path = PathBuf::from(value);
                Self::File(if path.is_absolute() {
                    path
                } else {
                    std::env::current_dir()
                        .expect("desktop working directory must be available")
                        .join(path)
                })
            }
        }
    }

    pub(crate) fn is_builtin(&self, name: &str) -> bool {
        matches!(self, Self::BuiltIn(selected) if selected == name)
    }

    pub(crate) fn is_file(&self) -> bool {
        matches!(self, Self::File(_))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ThemeSummary {
    pub(crate) name: String,
}

#[derive(Clone, Debug)]
pub(crate) struct CompiledTheme {
    name: String,
    colors: [u32; 10],
    product_font: SharedString,
    terminal_font: SharedString,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeFile {
    version: u16,
    name: String,
    colors: ThemeColorsFile,
    fonts: ThemeFontsFile,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeColorsFile {
    deck: String,
    panel: String,
    active: String,
    hairline: String,
    chalk: String,
    trace: String,
    relay: String,
    signal: String,
    success: String,
    fault: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeFontsFile {
    product: String,
    terminal: String,
}

#[derive(Debug, Error)]
pub(crate) enum ThemeError {
    #[error("theme extension manifest is invalid: {0}")]
    Manifest(#[from] ExtensionManifestError),
    #[error("unknown built-in theme {0:?}")]
    UnknownBuiltIn(String),
    #[error("theme file could not be read: {0}")]
    Io(#[from] std::io::Error),
    #[error("theme source must be a regular file and not a symlink")]
    InvalidFileType,
    #[error("theme file exceeds {MAX_THEME_BYTES} bytes")]
    TooLarge,
    #[error("theme JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported theme version {0}")]
    UnsupportedVersion(u16),
    #[error("theme name must contain 1 to {MAX_NAME_BYTES} bytes without control characters")]
    InvalidName,
    #[error("theme font {0:?} must contain 1 to {MAX_FONT_BYTES} bytes without control characters")]
    InvalidFont(String),
    #[error("theme color {field:?} must use exact #RRGGBB syntax")]
    InvalidColor { field: &'static str },
    #[error("theme colors {foreground:?} and {background:?} do not meet required contrast")]
    InsufficientContrast {
        foreground: &'static str,
        background: &'static str,
    },
}

struct ActiveTheme {
    colors: [AtomicU32; 10],
    fonts: RwLock<(SharedString, SharedString)>,
}

static ACTIVE_THEME: LazyLock<ActiveTheme> = LazyLock::new(|| {
    let theme = graphite();
    ActiveTheme {
        colors: theme.colors.map(AtomicU32::new),
        fonts: RwLock::new((theme.product_font, theme.terminal_font)),
    }
});

/// Data-only theme adapter seam. Implementations parse and validate outside paint.
pub(crate) trait ThemeSource: ExtensionAdapter {
    fn load(&self) -> Result<CompiledTheme, ThemeError>;
}

struct BuiltInSource<'a> {
    name: &'a str,
}

impl ExtensionAdapter for BuiltInSource<'_> {
    fn manifest(&self) -> ExtensionManifest<'_> {
        ExtensionManifest {
            id: "termi9ne.theme.builtin",
            version: THEME_FILE_VERSION,
            source: ExtensionSourceKind::Embedded,
            capabilities: &[ExtensionCapability::Theme],
        }
    }
}

impl ThemeSource for BuiltInSource<'_> {
    fn load(&self) -> Result<CompiledTheme, ThemeError> {
        match self.name {
            "graphite" => Ok(graphite()),
            "paper" => Ok(paper()),
            other => Err(ThemeError::UnknownBuiltIn(other.to_owned())),
        }
    }
}

struct FileSource<'a> {
    path: &'a Path,
}

impl ExtensionAdapter for FileSource<'_> {
    fn manifest(&self) -> ExtensionManifest<'_> {
        ExtensionManifest {
            id: "termi9ne.theme.file",
            version: THEME_FILE_VERSION,
            source: ExtensionSourceKind::DataFile,
            capabilities: &[ExtensionCapability::Theme],
        }
    }
}

impl ThemeSource for FileSource<'_> {
    fn load(&self) -> Result<CompiledTheme, ThemeError> {
        let metadata = fs::symlink_metadata(self.path)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(ThemeError::InvalidFileType);
        }
        if metadata.len() > MAX_THEME_BYTES {
            return Err(ThemeError::TooLarge);
        }
        let bytes = fs::read(self.path)?;
        let file: ThemeFile = serde_json::from_slice(&bytes)?;
        compile_file(file)
    }
}

pub(crate) fn activate(selection: &ThemeSelection) -> Result<ThemeSummary, ThemeError> {
    let compiled = match selection {
        ThemeSelection::BuiltIn(name) => load_source(&BuiltInSource { name })?,
        ThemeSelection::File(path) => load_source(&FileSource { path })?,
    };
    for (target, value) in ACTIVE_THEME.colors.iter().zip(compiled.colors) {
        target.store(value, Ordering::Relaxed);
    }
    *ACTIVE_THEME
        .fonts
        .write()
        .expect("active theme font lock must remain available") =
        (compiled.product_font, compiled.terminal_font);
    Ok(ThemeSummary {
        name: compiled.name,
    })
}

fn load_source(source: &impl ThemeSource) -> Result<CompiledTheme, ThemeError> {
    let manifest = source.manifest();
    validate_manifest(manifest)?;
    debug_assert_eq!(manifest.capabilities, &[ExtensionCapability::Theme]);
    source.load()
}

pub(crate) trait IntoThemeColor {
    fn resolve(self) -> u32;
}

impl IntoThemeColor for u32 {
    #[inline]
    fn resolve(self) -> u32 {
        self
    }
}

impl IntoThemeColor for ColorToken {
    #[inline]
    fn resolve(self) -> u32 {
        ACTIVE_THEME.colors[self as usize].load(Ordering::Relaxed)
    }
}

#[inline]
pub(crate) fn rgb(color: impl IntoThemeColor) -> Rgba {
    gpui::rgb(color.resolve())
}

impl From<FontToken> for SharedString {
    fn from(token: FontToken) -> Self {
        let fonts = ACTIVE_THEME
            .fonts
            .read()
            .expect("active theme font lock must remain available");
        match token {
            FontToken::Product => fonts.0.clone(),
            FontToken::Terminal => fonts.1.clone(),
        }
    }
}

fn compile_file(file: ThemeFile) -> Result<CompiledTheme, ThemeError> {
    if file.version != THEME_FILE_VERSION {
        return Err(ThemeError::UnsupportedVersion(file.version));
    }
    validate_label(&file.name, MAX_NAME_BYTES).map_err(|()| ThemeError::InvalidName)?;
    validate_font(&file.fonts.product)?;
    validate_font(&file.fonts.terminal)?;
    let colors = [
        parse_color("deck", &file.colors.deck)?,
        parse_color("panel", &file.colors.panel)?,
        parse_color("active", &file.colors.active)?,
        parse_color("hairline", &file.colors.hairline)?,
        parse_color("chalk", &file.colors.chalk)?,
        parse_color("trace", &file.colors.trace)?,
        parse_color("relay", &file.colors.relay)?,
        parse_color("signal", &file.colors.signal)?,
        parse_color("success", &file.colors.success)?,
        parse_color("fault", &file.colors.fault)?,
    ];
    validate_contrast(&colors)?;
    Ok(CompiledTheme {
        name: file.name,
        colors,
        product_font: file.fonts.product.into(),
        terminal_font: file.fonts.terminal.into(),
    })
}

fn validate_font(value: &str) -> Result<(), ThemeError> {
    validate_label(value, MAX_FONT_BYTES).map_err(|()| ThemeError::InvalidFont(value.to_owned()))
}

fn validate_label(value: &str, max: usize) -> Result<(), ()> {
    if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(());
    }
    Ok(())
}

fn parse_color(field: &'static str, value: &str) -> Result<u32, ThemeError> {
    let Some(hex) = value.strip_prefix('#') else {
        return Err(ThemeError::InvalidColor { field });
    };
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ThemeError::InvalidColor { field });
    }
    u32::from_str_radix(hex, 16).map_err(|_| ThemeError::InvalidColor { field })
}

fn validate_contrast(colors: &[u32; 10]) -> Result<(), ThemeError> {
    for (foreground, foreground_index, background, background_index, minimum) in [
        ("chalk", 4, "deck", 0, 4.5),
        ("chalk", 4, "panel", 1, 4.5),
        ("relay", 6, "deck", 0, 3.0),
        ("signal", 7, "deck", 0, 3.0),
        ("fault", 9, "deck", 0, 3.0),
    ] {
        if contrast_ratio(colors[foreground_index], colors[background_index]) < minimum {
            return Err(ThemeError::InsufficientContrast {
                foreground,
                background,
            });
        }
    }
    Ok(())
}

fn contrast_ratio(left: u32, right: u32) -> f32 {
    let left = relative_luminance(left);
    let right = relative_luminance(right);
    (left.max(right) + 0.05) / (left.min(right) + 0.05)
}

fn relative_luminance(color: u32) -> f32 {
    let channel = |shift: u32| {
        let value = u8::try_from((color >> shift) & 0xFF_u32)
            .expect("masked color channel must fit into u8");
        let normalized = f32::from(value) / 255.0;
        if normalized <= 0.040_45 {
            normalized / 12.92
        } else {
            ((normalized + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
}

fn graphite() -> CompiledTheme {
    CompiledTheme {
        name: "Graphite".to_owned(),
        colors: [
            0x272A31, 0x23262C, 0x343842, 0x505663, 0xD9DCE3, 0x858B9A, 0x8CB7E8, 0xD5B778,
            0x89C89B, 0xD77B72,
        ],
        product_font: "Familjen Grotesk".into(),
        terminal_font: "JetBrains Mono".into(),
    }
}

fn paper() -> CompiledTheme {
    CompiledTheme {
        name: "Paper".to_owned(),
        colors: [
            0xE9E7E1, 0xF5F3EE, 0xDAD7CF, 0xB8B4AA, 0x1F2329, 0x6F747C, 0x376C9F, 0x9B6C1E,
            0x357A4C, 0xA8463D,
        ],
        product_font: "Familjen Grotesk".into(),
        terminal_font: "JetBrains Mono".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_and_file_sources_share_one_validated_interface() {
        assert_eq!(
            load_source(&BuiltInSource { name: "paper" })
                .expect("built-in theme should load")
                .name,
            "Paper"
        );

        let root = std::env::temp_dir().join(format!("termi9ne-theme-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("theme fixture directory should exist");
        let path = root.join("theme.json");
        fs::write(
            &path,
            br##"{
                "version": 1,
                "name": "Control room",
                "colors": {
                    "deck": "#10151B", "panel": "#19212A", "active": "#24303C",
                    "hairline": "#334252", "chalk": "#E8EDF2", "trace": "#8795A5",
                    "relay": "#79A7D3", "signal": "#D8A85B", "success": "#7DB58B",
                    "fault": "#D7776B"
                },
                "fonts": { "product": "Familjen Grotesk", "terminal": "Iosevka Term" }
            }"##,
        )
        .expect("theme fixture should write");
        let source = FileSource { path: &path };
        assert_eq!(source.manifest().source, ExtensionSourceKind::DataFile);
        let theme = load_source(&source).expect("file theme should pass the same interface");
        assert_eq!(theme.name, "Control room");
        assert_eq!(theme.colors[0], 0x10151B);
        fs::remove_dir_all(root).expect("theme fixture should be removable");
    }

    #[test]
    fn invalid_theme_data_fails_before_activation() {
        let file = ThemeFile {
            version: 1,
            name: "Broken".to_owned(),
            colors: ThemeColorsFile {
                deck: "10151B".to_owned(),
                panel: "#19212A".to_owned(),
                active: "#24303C".to_owned(),
                hairline: "#334252".to_owned(),
                chalk: "#E8EDF2".to_owned(),
                trace: "#8795A5".to_owned(),
                relay: "#79A7D3".to_owned(),
                signal: "#D8A85B".to_owned(),
                success: "#7DB58B".to_owned(),
                fault: "#D7776B".to_owned(),
            },
            fonts: ThemeFontsFile {
                product: "Familjen Grotesk".to_owned(),
                terminal: "Iosevka Term".to_owned(),
            },
        };
        assert!(matches!(
            compile_file(file),
            Err(ThemeError::InvalidColor { field: "deck" })
        ));
    }

    #[test]
    fn activation_updates_cached_tokens_and_failure_keeps_the_active_theme() {
        activate(&ThemeSelection::BuiltIn("paper".to_owned())).expect("Paper should activate");
        assert_eq!(DECK.resolve(), 0xE9E7E1);
        assert!(matches!(
            activate(&ThemeSelection::BuiltIn("missing".to_owned())),
            Err(ThemeError::UnknownBuiltIn(_))
        ));
        assert_eq!(DECK.resolve(), 0xE9E7E1);
        activate(&ThemeSelection::default()).expect("Graphite should restore");
    }

    #[test]
    fn semantic_theme_pairs_must_meet_contrast_requirements() {
        let mut colors = graphite().colors;
        colors[4] = colors[0];
        assert!(matches!(
            validate_contrast(&colors),
            Err(ThemeError::InsufficientContrast {
                foreground: "chalk",
                background: "deck"
            })
        ));
    }
}
