use std::fs;

use serde::Deserialize;

use crate::atomic::write_atomic;
use crate::error::{PortaError, Result};
use crate::lock::{DEFAULT_LOCK_TIMEOUT, FileLock};
use crate::state::StatePaths;

use super::model::{REGISTRY_SCHEMA_VERSION, Registry, RegistryV1};

const MAX_REGISTRY_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Deserialize)]
struct SchemaHeader {
    schema_version: u32,
}

pub trait RegistryStore {
    fn begin(&self, initial_cleanup_trigger: usize) -> Result<RegistryTransaction>;
}

#[derive(Clone, Debug)]
pub struct JsonRegistryStore {
    paths: StatePaths,
}

impl JsonRegistryStore {
    #[must_use]
    pub fn new(paths: StatePaths) -> Self {
        Self { paths }
    }
}

impl RegistryStore for JsonRegistryStore {
    fn begin(&self, initial_cleanup_trigger: usize) -> Result<RegistryTransaction> {
        self.paths.ensure_root()?;
        let lock = FileLock::acquire(
            &self.paths.registry_lock,
            DEFAULT_LOCK_TIMEOUT,
            "registry_busy",
            "registry_lock_failed",
        )?;
        let (mut registry, migrated) = read_registry(&self.paths, initial_cleanup_trigger)?;
        let normalized = registry
            .maintenance
            .cleanup_trigger
            .clamp(initial_cleanup_trigger, 100);
        let dirty = migrated || normalized != registry.maintenance.cleanup_trigger;
        registry.maintenance.cleanup_trigger = normalized;
        Ok(RegistryTransaction {
            paths: self.paths.clone(),
            _lock: lock,
            registry,
            dirty,
        })
    }
}

pub struct RegistryTransaction {
    paths: StatePaths,
    _lock: FileLock,
    registry: Registry,
    dirty: bool,
}

impl RegistryTransaction {
    #[must_use]
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    pub fn registry_mut(&mut self) -> &mut Registry {
        self.dirty = true;
        &mut self.registry
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn commit(&mut self) -> Result<()> {
        if !self.dirty {
            return Ok(());
        }
        self.registry.validate()?;
        let serialized = serde_json::to_vec_pretty(&self.registry).map_err(|error| {
            PortaError::infrastructure("registry_write_failed", error.to_string())
        })?;
        let mut contents = serialized;
        contents.push(b'\n');
        write_atomic(&self.paths.registry, &contents, "registry_write_failed")?;
        self.dirty = false;
        Ok(())
    }
}

fn read_registry(paths: &StatePaths, initial_cleanup_trigger: usize) -> Result<(Registry, bool)> {
    let metadata = match fs::metadata(&paths.registry) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((Registry::empty(initial_cleanup_trigger), false));
        }
        Err(error) => {
            return Err(invalid_registry(format!(
                "Could not inspect {}: {error}",
                paths.registry.display()
            )));
        }
    };
    if metadata.len() > MAX_REGISTRY_BYTES {
        return Err(invalid_registry(format!(
            "Registry exceeds {MAX_REGISTRY_BYTES} bytes"
        )));
    }
    let contents = fs::read(&paths.registry).map_err(|error| {
        invalid_registry(format!(
            "Could not read {}: {error}",
            paths.registry.display()
        ))
    })?;
    let header: SchemaHeader = serde_json::from_slice(&contents)
        .map_err(|error| invalid_registry(format!("Could not parse registry: {error}")))?;
    let (registry, migrated) = match header.schema_version {
        REGISTRY_SCHEMA_VERSION => (
            serde_json::from_slice::<Registry>(&contents)
                .map_err(|error| invalid_registry(format!("Could not parse registry: {error}")))?,
            false,
        ),
        1 => {
            (
                Registry::from(serde_json::from_slice::<RegistryV1>(&contents).map_err(
                    |error| invalid_registry(format!("Could not parse registry: {error}")),
                )?),
                true,
            )
        }
        version => {
            return Err(invalid_registry(format!(
                "Unsupported registry schema version: {version}"
            )));
        }
    };
    registry.validate()?;
    Ok((registry, migrated))
}

fn invalid_registry(message: impl Into<String>) -> PortaError {
    PortaError::invalid("invalid_registry", message)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::Value;
    use tempfile::tempdir;

    use super::{JsonRegistryStore, RegistryStore};
    use crate::state::StatePaths;

    #[test]
    fn committed_registry_round_trips() {
        let temporary = tempdir().expect("temporary directory");
        let paths = StatePaths::from_root(temporary.path().join("state"));
        let store = JsonRegistryStore::new(paths);
        let mut transaction = store.begin(30).expect("begin transaction");
        transaction.registry_mut().maintenance.cleanup_trigger = 45;
        transaction.commit().expect("commit transaction");
        drop(transaction);

        let transaction = store.begin(30).expect("reopen transaction");
        assert_eq!(transaction.registry().maintenance.cleanup_trigger, 45);
    }

    #[test]
    fn version_one_registry_migrates_to_keyed_open_lease_schema() {
        let temporary = tempdir().expect("temporary directory");
        let paths = StatePaths::from_root(temporary.path().join("state"));
        paths.ensure_root().expect("state root");
        fs::write(
            &paths.registry,
            r#"{
  "schema_version": 1,
  "maintenance": {"cleanup_trigger": 30},
  "open_leases": [{
    "id": "28e5f42b-d26d-4974-9db9-b895433a94ef",
    "port": 62000,
    "created_at": "2026-07-21T19:30:30Z",
    "expires_at": "2026-07-21T19:32:30Z"
  }],
  "reservations": []
}"#,
        )
        .expect("version one registry");

        let store = JsonRegistryStore::new(paths.clone());
        let mut transaction = store.begin(30).expect("migrate registry");
        assert_eq!(transaction.registry().open_leases[0].key, None);
        transaction.commit().expect("persist migration");
        drop(transaction);

        let contents = fs::read(&paths.registry).expect("migrated registry");
        let value: Value = serde_json::from_slice(&contents).expect("valid JSON");
        assert_eq!(value["schema_version"], 2);
        assert!(value["open_leases"][0]["key"].is_null());
    }
}
