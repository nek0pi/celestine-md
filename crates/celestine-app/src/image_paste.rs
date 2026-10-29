//! Per-vault attachment preferences and clipboard image persistence.
use celestine_core::Vault;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;

#[derive(Clone, Deserialize, Serialize)]
pub struct Settings {
    #[serde(default = "default_folder")]
    pub images_folder: String,
    #[serde(flatten)]
    extra: std::collections::BTreeMap<String, toml::Value>,
}

fn default_folder() -> String {
    "images".into()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            images_folder: default_folder(),
            extra: Default::default(),
        }
    }
}

impl Settings {
    pub fn load(vault: &Vault) -> Result<Self, String> {
        match vault.read_metadata("settings.toml") {
            Ok(text) => toml::from_str(&text).map_err(|e| format!("invalid vault settings: {e}")),
            Err(celestine_core::CoreError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                Ok(Self::default())
            }
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn save_folder(vault: &Vault, folder: &str) -> Result<Self, String> {
        let folder = validate_folder(vault, folder)?;
        let mut settings = Self::load(vault)?;
        settings.images_folder = folder;
        let text = toml::to_string_pretty(&settings).map_err(|e| e.to_string())?;
        vault
            .write_metadata("settings.toml", &text)
            .map_err(|e| e.to_string())?;
        Ok(settings)
    }
}

pub fn validate_folder(vault: &Vault, folder: &str) -> Result<String, String> {
    let folder = folder.trim().replace('\\', "/");
    if folder.is_empty()
        || folder.starts_with('/')
        || folder.contains(':')
        || folder.split('/').any(|p| p.starts_with('.'))
    {
        return Err(
            "choose a visible folder relative to the vault, such as images or assets/images".into(),
        );
    }
    let folder = folder
        .split('/')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    let path = vault.abs_path(&folder).map_err(|e| e.to_string())?;
    if path.exists() && !path.is_dir() {
        return Err("the image folder points to a file".into());
    }
    Ok(folder)
}

/// Save before generating an embed. Exclusive creation protects existing attachments.
pub fn save_image(vault: &Vault, folder: &str, image: &egui::ColorImage) -> Result<String, String> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis();
    save_image_at(vault, folder, image, stamp)
}

fn save_image_at(
    vault: &Vault,
    folder: &str,
    image: &egui::ColorImage,
    stamp: u128,
) -> Result<String, String> {
    let folder = validate_folder(vault, folder)?;
    let width = u32::try_from(image.width()).map_err(|e| e.to_string())?;
    let height = u32::try_from(image.height()).map_err(|e| e.to_string())?;
    if width == 0
        || height == 0
        || image.width().checked_mul(image.height()) != Some(image.pixels.len())
    {
        return Err("clipboard image has invalid dimensions".into());
    }
    let rgba = image
        .pixels
        .iter()
        .flat_map(|p| p.to_srgba_unmultiplied())
        .collect::<Vec<_>>();
    let mut png = Vec::new();
    image::ImageEncoder::write_image(
        image::codecs::png::PngEncoder::new(&mut png),
        &rgba,
        width,
        height,
        image::ExtendedColorType::Rgba8,
    )
    .map_err(|e| e.to_string())?;
    let dir = vault.abs_path(&folder).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // Resolve again after creating parents, including any existing symlinks.
    validate_folder(vault, &folder)?;
    for suffix in 0u64.. {
        let name = if suffix == 0 {
            format!("pasted-image-{stamp}.png")
        } else {
            format!("pasted-image-{stamp}-{suffix}.png")
        };
        let rel = format!("{folder}/{name}");
        let path = vault.abs_path(&rel).map_err(|e| e.to_string())?;
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                if let Err(e) = file.write_all(&png).and_then(|_| file.sync_all()) {
                    drop(file);
                    let _ = std::fs::remove_file(&path);
                    return Err(e.to_string());
                }
                return Ok(rel);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    unreachable!()
}

pub fn embed(note: &str, image: &str) -> String {
    let parent = Path::new(note).parent().unwrap_or(Path::new(""));
    let parents = parent
        .components()
        .filter_map(|p| p.as_os_str().to_str())
        .collect::<Vec<_>>();
    let image_parts = image.split('/').collect::<Vec<_>>();
    let common = parents
        .iter()
        .zip(&image_parts)
        .take_while(|(a, b)| a == b)
        .count();
    let relative = std::iter::repeat_n("..", parents.len() - common)
        .chain(image_parts[common..].iter().copied())
        .collect::<Vec<_>>()
        .join("/");
    // Angle destinations handle parentheses; percent escapes handle whitespace and delimiters.
    let mut escaped = String::new();
    for c in relative.chars() {
        match c {
            '%' => escaped.push_str("%25"),
            ' ' => escaped.push_str("%20"),
            '<' => escaped.push_str("%3C"),
            '>' => escaped.push_str("%3E"),
            '#' => escaped.push_str("%23"),
            '?' => escaped.push_str("%3F"),
            c if c.is_control() => {
                for b in c.to_string().bytes() {
                    escaped.push_str(&format!("%{b:02X}"));
                }
            }
            c => escaped.push(c),
        }
    }
    format!("\n![](<{escaped}>)\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_pastes_preserve_alpha_and_never_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        let image =
            egui::ColorImage::from_rgba_unmultiplied([2, 1], &[255, 255, 255, 128, 255, 0, 0, 255]);
        let first = save_image_at(&vault, "assets/images", &image, 123).unwrap();
        let second = save_image_at(&vault, "assets/images", &image, 123).unwrap();
        assert_eq!(first, "assets/images/pasted-image-123.png");
        assert_eq!(second, "assets/images/pasted-image-123-1.png");
        let decoded = image::open(dir.path().join(&first)).unwrap().into_rgba8();
        assert_eq!(decoded.dimensions(), (2, 1));
        assert_eq!(decoded.into_raw(), [255, 255, 255, 128, 255, 0, 0, 255]);
    }

    #[test]
    fn settings_are_per_vault_and_preserve_other_fields() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let a = Vault::open(a.path()).unwrap();
        let b = Vault::open(b.path()).unwrap();
        assert_eq!(Settings::load(&a).unwrap().images_folder, "images");
        a.write_metadata("settings.toml", "future_setting = true\n")
            .unwrap();
        Settings::save_folder(&a, " assets/screenshots/ ").unwrap();
        assert_eq!(
            Settings::load(&a).unwrap().images_folder,
            "assets/screenshots"
        );
        assert_eq!(Settings::load(&b).unwrap().images_folder, "images");
        assert!(a
            .read_metadata("settings.toml")
            .unwrap()
            .contains("future_setting = true"));
        assert!(!a.root().join("assets").exists());
    }

    #[test]
    fn invalid_folders_and_failed_writes_do_not_create_images() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        let image = egui::ColorImage::filled([1, 1], egui::Color32::WHITE);
        for folder in [
            "",
            "../outside",
            "/tmp/images",
            ".hidden",
            "assets/../images",
            "C:\\images",
        ] {
            assert!(
                save_image_at(&vault, folder, &image, 123).is_err(),
                "{folder}"
            );
        }
        std::fs::write(dir.path().join("blocked"), "sentinel").unwrap();
        assert!(save_image_at(&vault, "blocked/nested", &image, 123).is_err());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("blocked")).unwrap(),
            "sentinel"
        );
        assert!(vault.list_image_rel_paths().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn image_and_settings_writes_reject_external_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let vault = Vault::open(root.path()).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("images")).unwrap();
        let image = egui::ColorImage::filled([1, 1], egui::Color32::WHITE);
        assert!(save_image_at(&vault, "images", &image, 123).is_err());
        std::os::unix::fs::symlink(outside.path(), root.path().join(".celestine")).unwrap();
        assert!(Settings::save_folder(&vault, "assets").is_err());
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    }

    #[test]
    fn nested_note_embeds_resolve_special_folder_names() {
        let root = tempfile::tempdir().unwrap();
        let vault = Vault::open(root.path()).unwrap();
        let image = egui::ColorImage::filled([1, 1], egui::Color32::WHITE);
        let rel = save_image_at(&vault, "assets/my pictures (#1) %20", &image, 123).unwrap();
        for note in ["Home.md", "notes/nested/Note.md", "assets/Note.md"] {
            let md = embed(note, &rel);
            let dest = pulldown_cmark::Parser::new(&md)
                .find_map(|e| match e {
                    pulldown_cmark::Event::Start(pulldown_cmark::Tag::Image {
                        dest_url, ..
                    }) => Some(dest_url.into_string()),
                    _ => None,
                })
                .unwrap();
            assert_eq!(
                crate::media::resolve_image_abs(vault.root(), note, &dest, &[]),
                Some(root.path().join(&rel))
            );
        }
    }
}
