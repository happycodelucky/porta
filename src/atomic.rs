use std::fs::File;
use std::io::Write;
use std::path::Path;

use tempfile::NamedTempFile;

use crate::error::{PortaError, Result};

pub fn write_atomic(path: &Path, bytes: &[u8], error_code: &'static str) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        PortaError::infrastructure(error_code, format!("{} has no parent", path.display()))
    })?;
    let mut temporary = NamedTempFile::new_in(parent).map_err(|error| {
        PortaError::infrastructure(error_code, format!("Could not stage write: {error}"))
    })?;
    set_file_permissions(temporary.path(), error_code)?;
    temporary.write_all(bytes).map_err(|error| {
        PortaError::infrastructure(error_code, format!("Could not write staged file: {error}"))
    })?;
    temporary.as_file().sync_all().map_err(|error| {
        PortaError::infrastructure(error_code, format!("Could not sync staged file: {error}"))
    })?;
    temporary.persist(path).map_err(|error| {
        PortaError::infrastructure(error_code, format!("Could not commit staged file: {error}"))
    })?;
    sync_parent(parent, error_code)
}

fn sync_parent(parent: &Path, error_code: &'static str) -> Result<()> {
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            PortaError::infrastructure(
                error_code,
                format!("Could not sync {}: {error}", parent.display()),
            )
        })
}

#[cfg(unix)]
fn set_file_permissions(path: &Path, error_code: &'static str) -> Result<()> {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|error| {
        PortaError::infrastructure(
            error_code,
            format!("Could not secure {}: {error}", path.display()),
        )
    })
}

#[cfg(not(unix))]
fn set_file_permissions(_path: &Path, _error_code: &'static str) -> Result<()> {
    Ok(())
}
