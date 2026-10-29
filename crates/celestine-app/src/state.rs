use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppState {
    /// Last successfully opened vault (auto-restored on launch).
    #[serde(default)]
    pub last_vault: Option<String>,
    #[serde(default)]
    pub recent_vaults: Vec<String>,
    #[serde(default = "default_ui_scale")]
    pub ui_font_scale: f32,
    #[serde(default = "default_editor_size")]
    pub editor_font_size: f32,
    /// Horizontal padding (px) on each side of the centered note column.
    #[serde(default = "default_note_h_pad")]
    pub note_h_pad: f32,
    #[serde(default)]
    pub theme: Option<String>,
    #[serde(default = "default_true")]
    pub show_left_sidebar: bool,
    #[serde(default = "default_true")]
    pub show_right_sidebar: bool,
    /// Last dismissed feature-line update (`"MAJOR.MINOR"`), so patch bumps
    /// and the same feature release don't re-nag after dismiss.
    #[serde(default)]
    pub dismissed_update_line: Option<String>,
    /// Show Git status and sync for vaults that are repository roots.
    #[serde(default = "default_true")]
    pub git_integration: bool,
}

fn default_ui_scale() -> f32 {
    1.10
}

fn default_editor_size() -> f32 {
    15.4 // ~14 * 1.10
}

fn default_note_h_pad() -> f32 {
    120.0
}

fn default_true() -> bool {
    true
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            last_vault: None,
            recent_vaults: Vec::new(),
            ui_font_scale: default_ui_scale(),
            editor_font_size: default_editor_size(),
            note_h_pad: default_note_h_pad(),
            theme: None,
            show_left_sidebar: true,
            show_right_sidebar: true,
            dismissed_update_line: None,
            git_integration: true,
        }
    }
}

impl AppState {
    fn path() -> Option<PathBuf> {
        directories::ProjectDirs::from("md", "Celestine", "celestine")
            .map(|d| d.config_dir().join("state.toml"))
    }

    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        toml::from_str(&text).unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(path) = Self::path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = toml::to_string_pretty(self) {
            let _ = std::fs::write(path, text);
        }
    }

    pub fn push_recent(&mut self, vault: &Path) {
        let s = vault.display().to_string();
        self.last_vault = Some(s.clone());
        self.recent_vaults.retain(|p| p != &s);
        self.recent_vaults.insert(0, s);
        self.recent_vaults.truncate(12);
        self.save();
    }

    pub fn clear_recent_missing(&mut self) {
        let before_n = self.recent_vaults.len();
        let before_last = self.last_vault.clone();
        self.recent_vaults.retain(|p| Path::new(p).is_dir());
        if let Some(last) = &self.last_vault {
            if !Path::new(last).is_dir() {
                self.last_vault = None;
            }
        }
        if self.recent_vaults.len() != before_n || self.last_vault != before_last {
            self.save();
        }
    }

    /// Preferred vault to restore on launch.
    pub fn vault_to_restore(&self) -> Option<PathBuf> {
        if let Some(last) = &self.last_vault {
            let p = PathBuf::from(last);
            if p.is_dir() {
                return Some(p);
            }
        }
        self.recent_vaults
            .iter()
            .map(PathBuf::from)
            .find(|p| p.is_dir())
    }
}
