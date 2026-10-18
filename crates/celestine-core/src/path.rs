use crate::CoreError;
use std::path::{Component, Path, PathBuf};

/// Resolve a vault-relative path while ensuring existing filesystem entries stay
/// within the canonical vault root. Missing suffixes are allowed for creation.
pub fn resolve_vault_path(root: &Path, rel: &Path) -> Result<PathBuf, CoreError> {
    Ok(resolve_vault_rel(root, rel)?.path)
}

pub(crate) struct ResolvedVaultPath {
    pub path: PathBuf,
    canonical_root: PathBuf,
}

pub(crate) fn resolve_vault_rel(root: &Path, rel: &Path) -> Result<ResolvedVaultPath, CoreError> {
    let canonical_root = root.canonicalize()?;
    let components = normalized_components(rel)?;
    resolve_components(canonical_root, components, rel)
}

pub(crate) fn resolve_vault_abs(root: &Path, path: &Path) -> Result<ResolvedVaultPath, CoreError> {
    let canonical_root = root.canonicalize()?;
    let rel = relative_to_root(&canonical_root, path)?;
    let components = normalized_components(Path::new(&rel))?;
    resolve_components(canonical_root, components, path)
}

pub(crate) fn checked_rel(root: &Path, path: &Path) -> Result<String, CoreError> {
    let canonical_root = root.canonicalize()?;
    relative_to_root(&canonical_root, path)
}

pub(crate) fn canonical_note_target(
    resolved: ResolvedVaultPath,
) -> Result<(PathBuf, String), CoreError> {
    let target = resolved.path.canonicalize()?;
    let rel = relative_to_root(&resolved.canonical_root, &target)?;
    Ok((target, rel))
}

fn relative_to_root(root: &Path, path: &Path) -> Result<String, CoreError> {
    if !path.is_absolute() {
        return Err(invalid_path(path, "absolute path required"));
    }
    #[cfg(not(windows))]
    let rel = if let Ok(rel) = path.strip_prefix(root) {
        rel.to_path_buf()
    } else {
        identity_relative(root, path)?
    };
    #[cfg(windows)]
    let rel = identity_relative(root, path)?;
    let rel = rel.to_string_lossy().replace('\\', "/");
    if rel.is_empty() {
        return Err(invalid_path(path, "vault root is not a file path"));
    }
    Ok(rel)
}

fn identity_relative(root: &Path, path: &Path) -> Result<PathBuf, CoreError> {
    let prefix = path
        .ancestors()
        .find(|candidate| candidate.is_dir() && same_location(root, candidate))
        .ok_or_else(|| invalid_path(path, "path is outside the vault"))?;
    path.strip_prefix(prefix)
        .map(Path::to_path_buf)
        .map_err(|_| invalid_path(path, "path is outside the vault"))
}

fn same_location(left: &Path, right: &Path) -> bool {
    left == right || same_file::is_same_file(left, right).unwrap_or(false)
}

fn normalized_components(rel: &Path) -> Result<Vec<std::ffi::OsString>, CoreError> {
    let raw = rel.to_string_lossy().replace('\\', "/");
    if cfg!(windows)
        && raw.as_bytes().get(1) == Some(&b':')
        && raw.as_bytes()[0].is_ascii_alphabetic()
    {
        return Err(invalid_path(rel, "absolute paths are not allowed"));
    }
    let path = Path::new(&raw);
    let mut components = Vec::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => components.push(part.to_os_string()),
            Component::ParentDir => {
                if components.pop().is_none() {
                    return Err(invalid_path(rel, "path escapes the vault"));
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(invalid_path(rel, "absolute paths are not allowed"));
            }
        }
    }
    if components.is_empty() {
        return Err(invalid_path(rel, "vault root is not a file path"));
    }

    Ok(components)
}

fn resolve_components(
    canonical_root: PathBuf,
    components: Vec<std::ffi::OsString>,
    original: &Path,
) -> Result<ResolvedVaultPath, CoreError> {
    let mut normalized = canonical_root.clone();
    for component in &components {
        normalized.push(component);
    }

    // Inspect each existing prefix. symlink_metadata sees dangling links, whose
    // canonicalization fails instead of treating the final path as missing.
    let mut prefix = canonical_root.clone();
    for component in &components {
        prefix.push(component);
        match std::fs::symlink_metadata(&prefix) {
            Ok(_) => {
                let resolved = prefix.canonicalize().map_err(|err| {
                    CoreError::InvalidVaultPath(format!("{}: {err}", original.display()))
                })?;
                if resolved != canonical_root
                    && relative_to_root(&canonical_root, &resolved).is_err()
                    && !same_location(&canonical_root, &resolved)
                {
                    return Err(invalid_path(original, "resolved path is outside the vault"));
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => break,
            Err(err) => return Err(err.into()),
        }
    }

    Ok(ResolvedVaultPath {
        path: normalized,
        canonical_root,
    })
}

fn invalid_path(path: &Path, reason: &str) -> CoreError {
    CoreError::InvalidVaultPath(format!("{}: {reason}", path.display()))
}
