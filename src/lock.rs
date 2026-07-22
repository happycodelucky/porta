use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use crate::error::{PortaError, Result};

pub const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_secs(5);

pub struct FileLock {
    file: File,
}

impl FileLock {
    pub fn acquire(
        path: &Path,
        timeout: Duration,
        busy_code: &'static str,
        failed_code: &'static str,
    ) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|error| {
                PortaError::infrastructure(
                    failed_code,
                    format!("Could not open {}: {error}", path.display()),
                )
            })?;
        set_file_permissions(path, failed_code)?;

        let started = Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self { file }),
                Err(TryLockError::WouldBlock) => {
                    if started.elapsed() >= timeout {
                        return Err(PortaError::infrastructure(
                            busy_code,
                            format!("Timed out waiting for {}", path.display()),
                        ));
                    }
                    thread::sleep(Duration::from_millis(25));
                }
                Err(TryLockError::Error(error)) => {
                    return Err(PortaError::infrastructure(
                        failed_code,
                        format!("Could not lock {}: {error}", path.display()),
                    ));
                }
            }
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[cfg(unix)]
fn set_file_permissions(path: &Path, failed_code: &'static str) -> Result<()> {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|error| {
        PortaError::infrastructure(
            failed_code,
            format!("Could not secure {}: {error}", path.display()),
        )
    })
}

#[cfg(not(unix))]
fn set_file_permissions(_path: &Path, _failed_code: &'static str) -> Result<()> {
    Ok(())
}
