use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::error::{PortaError, Result};

#[derive(Clone, Debug)]
pub struct StatePaths {
    pub root: PathBuf,
    pub config: PathBuf,
    pub config_lock: PathBuf,
    pub registry: PathBuf,
    pub registry_lock: PathBuf,
}

impl StatePaths {
    pub fn discover() -> Result<Self> {
        if let Some(root) = env::var_os("PORTA_HOME") {
            return Ok(Self::from_root(PathBuf::from(root)));
        }
        let home = env::var_os("HOME").ok_or_else(|| {
            PortaError::infrastructure(
                "state_directory_failed",
                "HOME is not set and PORTA_HOME was not provided",
            )
        })?;
        Ok(Self::from_root(PathBuf::from(home).join(".porta")))
    }

    #[must_use]
    pub fn from_root(root: PathBuf) -> Self {
        Self {
            config: root.join("config.toml"),
            config_lock: root.join("config.lock"),
            registry: root.join("registry.json"),
            registry_lock: root.join("registry.lock"),
            root,
        }
    }

    pub fn ensure_root(&self) -> Result<()> {
        fs::create_dir_all(&self.root).map_err(|error| {
            PortaError::infrastructure(
                "state_directory_failed",
                format!("Could not create {}: {error}", self.root.display()),
            )
        })?;
        if !self.root.is_dir() {
            return Err(PortaError::infrastructure(
                "state_directory_failed",
                format!("State path is not a directory: {}", self.root.display()),
            ));
        }
        set_directory_permissions(&self.root)?;
        Ok(())
    }
}

pub fn resolve_directory(path: &Path, must_exist: bool) -> Result<PathBuf> {
    let expanded = expand_home(path)?;
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        env::current_dir()
            .map_err(|error| PortaError::infrastructure("directory_failed", error.to_string()))?
            .join(expanded)
    };

    if must_exist {
        let resolved = fs::canonicalize(&absolute).map_err(|error| {
            PortaError::invalid(
                "invalid_arguments",
                format!(
                    "Reservation directory does not exist: {} ({error})",
                    absolute.display()
                ),
            )
        })?;
        if !resolved.is_dir() {
            return Err(PortaError::invalid(
                "invalid_arguments",
                format!(
                    "Reservation path is not a directory: {}",
                    resolved.display()
                ),
            ));
        }
        ensure_utf8(&resolved)?;
        return Ok(resolved);
    }

    let resolved = canonicalize_existing_prefix(&absolute)?;
    ensure_utf8(&resolved)?;
    Ok(resolved)
}

fn expand_home(path: &Path) -> Result<PathBuf> {
    let mut components = path.components();
    if components.next() != Some(Component::Normal("~".as_ref())) {
        return Ok(path.to_path_buf());
    }
    let home = env::var_os("HOME")
        .ok_or_else(|| PortaError::invalid("invalid_arguments", "HOME is not set"))?;
    Ok(PathBuf::from(home).join(components.as_path()))
}

fn canonicalize_existing_prefix(path: &Path) -> Result<PathBuf> {
    let mut existing = path.to_path_buf();
    let mut suffix: Vec<OsString> = Vec::new();
    while !existing.exists() {
        let name = existing.file_name().ok_or_else(|| {
            PortaError::invalid(
                "invalid_arguments",
                format!("Could not resolve directory path: {}", path.display()),
            )
        })?;
        suffix.push(name.to_os_string());
        existing.pop();
    }

    let mut resolved = fs::canonicalize(&existing).map_err(|error| {
        PortaError::infrastructure(
            "directory_failed",
            format!("Could not resolve {}: {error}", existing.display()),
        )
    })?;
    for component in suffix.into_iter().rev() {
        if component == "." {
            continue;
        }
        if component == ".." {
            resolved.pop();
        } else {
            resolved.push(component);
        }
    }
    Ok(resolved)
}

fn ensure_utf8(path: &Path) -> Result<()> {
    if path.to_str().is_none() {
        return Err(PortaError::invalid(
            "invalid_arguments",
            "Directory paths must be valid UTF-8",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn set_directory_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|error| {
        PortaError::infrastructure(
            "state_directory_failed",
            format!("Could not secure {}: {error}", path.display()),
        )
    })
}

#[cfg(not(unix))]
fn set_directory_permissions(_path: &Path) -> Result<()> {
    Ok(())
}
