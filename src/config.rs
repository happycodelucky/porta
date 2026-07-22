use std::fs;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::atomic::write_atomic;
use crate::duration::{format_duration, parse_duration};
use crate::error::{PortaError, Result};
use crate::lock::{DEFAULT_LOCK_TIMEOUT, FileLock};
use crate::state::StatePaths;

const MAX_CONFIG_BYTES: u64 = 64 * 1024;
const SUPPORTED_CONFIG_KEYS: [&str; 5] = [
    "default_port",
    "lease_timeout",
    "missing_for",
    "cleanup_trigger_start",
    "cleanup_trigger_step",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    pub default_port: u16,
    pub lease_timeout: Duration,
    pub missing_for: Duration,
    pub cleanup_trigger_start: usize,
    pub cleanup_trigger_step: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_port: 55_000,
            lease_timeout: Duration::from_mins(2),
            missing_for: Duration::from_hours(7 * 24),
            cleanup_trigger_start: 30,
            cleanup_trigger_step: 15,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ConfigDocument {
    #[serde(skip_serializing_if = "Option::is_none")]
    default_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lease_timeout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    missing_for: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cleanup_trigger_start: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cleanup_trigger_step: Option<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ConfigValue {
    Integer(u64),
    Duration(String),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ConfigEntry {
    pub key: &'static str,
    pub value: ConfigValue,
    pub default: ConfigValue,
    pub is_set: bool,
}

impl ConfigValue {
    #[must_use]
    pub fn plain(&self) -> String {
        match self {
            Self::Integer(value) => value.to_string(),
            Self::Duration(value) => value.clone(),
        }
    }

    #[must_use]
    pub fn json(&self) -> Value {
        match self {
            Self::Integer(value) => Value::from(*value),
            Self::Duration(value) => Value::from(value.clone()),
        }
    }
}

pub struct ConfigStore {
    paths: StatePaths,
}

impl ConfigStore {
    #[must_use]
    pub fn new(paths: StatePaths) -> Self {
        Self { paths }
    }

    pub fn load(&self) -> Result<Config> {
        load_path(&self.paths)
    }

    pub fn get(&self, key: &str) -> Result<ConfigValue> {
        self.load()?.get(key)
    }

    pub fn list(&self) -> Result<Vec<ConfigEntry>> {
        let document = load_document(&self.paths)?;
        document.entries()
    }

    pub fn set(&self, key: &str, value: &str) -> Result<ConfigValue> {
        self.paths.ensure_root()?;
        let _lock = FileLock::acquire(
            &self.paths.config_lock,
            DEFAULT_LOCK_TIMEOUT,
            "config_busy",
            "config_write_failed",
        )?;
        let mut document = load_document(&self.paths)?;
        let selected = document.set(key, value)?;
        let serialized = toml::to_string_pretty(&document).map_err(|error| {
            PortaError::infrastructure("config_write_failed", error.to_string())
        })?;
        write_atomic(
            &self.paths.config,
            serialized.as_bytes(),
            "config_write_failed",
        )?;
        Ok(selected)
    }
}

impl ConfigDocument {
    fn entries(&self) -> Result<Vec<ConfigEntry>> {
        let effective = validate_document(self)?;
        let defaults = Config::default();
        SUPPORTED_CONFIG_KEYS
            .into_iter()
            .map(|key| {
                Ok(ConfigEntry {
                    key,
                    value: effective.get(key)?,
                    default: defaults.get(key)?,
                    is_set: self.is_set(key),
                })
            })
            .collect()
    }

    fn is_set(&self, key: &str) -> bool {
        match key {
            "default_port" => self.default_port.is_some(),
            "lease_timeout" => self.lease_timeout.is_some(),
            "missing_for" => self.missing_for.is_some(),
            "cleanup_trigger_start" => self.cleanup_trigger_start.is_some(),
            "cleanup_trigger_step" => self.cleanup_trigger_step.is_some(),
            _ => false,
        }
    }

    fn set(&mut self, key: &str, value: &str) -> Result<ConfigValue> {
        let mut effective = validate_document(self)?;
        effective.set(key, value)?;
        let selected = effective.get(key)?;
        match (key, &selected) {
            ("default_port", ConfigValue::Integer(value)) => {
                self.default_port = Some(u16::try_from(*value).map_err(|_| invalid_value(key))?);
            }
            ("lease_timeout", ConfigValue::Duration(value)) => {
                self.lease_timeout = Some(value.clone());
            }
            ("missing_for", ConfigValue::Duration(value)) => {
                self.missing_for = Some(value.clone());
            }
            ("cleanup_trigger_start", ConfigValue::Integer(value)) => {
                self.cleanup_trigger_start =
                    Some(usize::try_from(*value).map_err(|_| invalid_value(key))?);
            }
            ("cleanup_trigger_step", ConfigValue::Integer(value)) => {
                self.cleanup_trigger_step =
                    Some(usize::try_from(*value).map_err(|_| invalid_value(key))?);
            }
            _ => return Err(invalid_value(key)),
        }
        Ok(selected)
    }
}

impl Config {
    pub fn get(&self, key: &str) -> Result<ConfigValue> {
        match key {
            "default_port" => Ok(ConfigValue::Integer(u64::from(self.default_port))),
            "lease_timeout" => Ok(ConfigValue::Duration(format_duration(self.lease_timeout))),
            "missing_for" => Ok(ConfigValue::Duration(format_duration(self.missing_for))),
            "cleanup_trigger_start" => Ok(ConfigValue::Integer(
                u64::try_from(self.cleanup_trigger_start).unwrap_or(u64::MAX),
            )),
            "cleanup_trigger_step" => Ok(ConfigValue::Integer(
                u64::try_from(self.cleanup_trigger_step).unwrap_or(u64::MAX),
            )),
            _ => Err(PortaError::invalid(
                "invalid_config_key",
                format!("Unknown configuration key: {key}"),
            )),
        }
    }

    fn set(&mut self, key: &str, value: &str) -> Result<()> {
        match key {
            "default_port" => {
                let port = value.parse::<u16>().map_err(|_| invalid_value(key))?;
                if port == 0 {
                    return Err(invalid_value(key));
                }
                self.default_port = port;
            }
            "lease_timeout" => {
                self.lease_timeout =
                    parse_duration(value, true, false).map_err(|_| invalid_value(key))?;
            }
            "missing_for" => {
                self.missing_for =
                    parse_duration(value, false, true).map_err(|_| invalid_value(key))?;
            }
            "cleanup_trigger_start" => {
                let trigger = value.parse::<usize>().map_err(|_| invalid_value(key))?;
                if !(1..=100).contains(&trigger) {
                    return Err(invalid_value(key));
                }
                self.cleanup_trigger_start = trigger;
            }
            "cleanup_trigger_step" => {
                let step = value.parse::<usize>().map_err(|_| invalid_value(key))?;
                if !(10..=15).contains(&step) {
                    return Err(invalid_value(key));
                }
                self.cleanup_trigger_step = step;
            }
            _ => {
                return Err(PortaError::invalid(
                    "invalid_config_key",
                    format!("Unknown configuration key: {key}"),
                ));
            }
        }
        Ok(())
    }
}

fn load_path(paths: &StatePaths) -> Result<Config> {
    validate_document(&load_document(paths)?)
}

fn load_document(paths: &StatePaths) -> Result<ConfigDocument> {
    let metadata = match fs::metadata(&paths.config) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ConfigDocument::default());
        }
        Err(error) => {
            return Err(PortaError::invalid(
                "invalid_config",
                format!("Could not inspect {}: {error}", paths.config.display()),
            ));
        }
    };
    if metadata.len() > MAX_CONFIG_BYTES {
        return Err(PortaError::invalid(
            "invalid_config",
            format!("Configuration exceeds {MAX_CONFIG_BYTES} bytes"),
        ));
    }
    let text = fs::read_to_string(&paths.config).map_err(|error| {
        PortaError::invalid(
            "invalid_config",
            format!("Could not read {}: {error}", paths.config.display()),
        )
    })?;
    toml::from_str(&text).map_err(|error| {
        PortaError::invalid(
            "invalid_config",
            format!("Could not parse {}: {error}", paths.config.display()),
        )
    })
}

fn validate_document(document: &ConfigDocument) -> Result<Config> {
    let mut config = Config::default();
    if let Some(default_port) = document.default_port {
        if default_port == 0 {
            return Err(invalid_config("default_port must be between 1 and 65535"));
        }
        config.default_port = default_port;
    }
    if let Some(lease_timeout) = &document.lease_timeout {
        config.lease_timeout = parse_duration(lease_timeout, true, false)
            .map_err(|error| invalid_config(error.message))?;
    }
    if let Some(missing_for) = &document.missing_for {
        config.missing_for = parse_duration(missing_for, false, true)
            .map_err(|error| invalid_config(error.message))?;
    }
    if let Some(cleanup_trigger_start) = document.cleanup_trigger_start {
        if !(1..=100).contains(&cleanup_trigger_start) {
            return Err(invalid_config(
                "cleanup_trigger_start must be between 1 and 100",
            ));
        }
        config.cleanup_trigger_start = cleanup_trigger_start;
    }
    if let Some(cleanup_trigger_step) = document.cleanup_trigger_step {
        if !(10..=15).contains(&cleanup_trigger_step) {
            return Err(invalid_config(
                "cleanup_trigger_step must be between 10 and 15",
            ));
        }
        config.cleanup_trigger_step = cleanup_trigger_step;
    }
    Ok(config)
}

fn invalid_config(message: impl Into<String>) -> PortaError {
    PortaError::invalid("invalid_config", message)
}

fn invalid_value(key: &str) -> PortaError {
    PortaError::invalid(
        "invalid_config_value",
        format!("Invalid value for configuration key: {key}"),
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{Config, ConfigStore, ConfigValue};
    use crate::state::StatePaths;

    #[test]
    fn missing_file_uses_defaults() {
        let root = tempdir().expect("temporary directory");
        let store = ConfigStore::new(StatePaths::from_root(root.path().join("state")));
        assert_eq!(store.load().expect("defaults"), Config::default());
        let entries = store.list().expect("default listing");
        assert_eq!(entries.len(), 5);
        assert!(entries.iter().all(|entry| !entry.is_set));
    }

    #[test]
    fn set_round_trips_canonical_values() {
        let root = tempdir().expect("temporary directory");
        let paths = StatePaths::from_root(root.path().join("state"));
        let store = ConfigStore::new(paths.clone());
        assert_eq!(
            store.set("missing_for", "1w2d").expect("set should work"),
            ConfigValue::Duration("1w2d".to_owned())
        );
        assert_eq!(
            store.get("missing_for").expect("get should work"),
            ConfigValue::Duration("1w2d".to_owned())
        );
        let contents = fs::read_to_string(paths.config).expect("config should exist");
        assert!(contents.contains("missing_for = \"1w2d\""));
        assert!(!contents.contains("default_port"));
        assert!(!contents.contains("lease_timeout"));
        assert!(!contents.contains("cleanup_trigger_start"));
        assert!(!contents.contains("cleanup_trigger_step"));

        let entries = store.list().expect("configured listing");
        assert!(entries[2].is_set);
        assert!(
            entries
                .iter()
                .enumerate()
                .all(|(index, entry)| index == 2 || !entry.is_set)
        );
    }

    #[test]
    fn explicitly_setting_a_default_value_is_still_set() {
        let root = tempdir().expect("temporary directory");
        let store = ConfigStore::new(StatePaths::from_root(root.path().join("state")));
        store
            .set("default_port", "55000")
            .expect("set explicit default");

        let entries = store.list().expect("configured listing");
        assert_eq!(entries[0].value, entries[0].default);
        assert!(entries[0].is_set);
        assert!(entries[1..].iter().all(|entry| !entry.is_set));
    }

    #[test]
    fn partial_and_legacy_full_documents_preserve_presence() {
        let root = tempdir().expect("temporary directory");
        let paths = StatePaths::from_root(root.path().join("state"));
        fs::create_dir_all(&paths.root).expect("state directory");
        fs::write(&paths.config, "lease_timeout = \"5m\"\n").expect("partial config");
        let store = ConfigStore::new(paths.clone());
        let partial = store.list().expect("partial listing");
        assert!(partial[1].is_set);
        assert!(
            partial
                .iter()
                .enumerate()
                .all(|(index, entry)| index == 1 || !entry.is_set)
        );

        fs::write(
            &paths.config,
            concat!(
                "default_port = 55000\n",
                "lease_timeout = \"2m\"\n",
                "missing_for = \"1w\"\n",
                "cleanup_trigger_start = 30\n",
                "cleanup_trigger_step = 15\n",
            ),
        )
        .expect("legacy full config");
        assert!(
            store
                .list()
                .expect("legacy listing")
                .iter()
                .all(|entry| entry.is_set)
        );
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let root = tempdir().expect("temporary directory");
        let paths = StatePaths::from_root(root.path().join("state"));
        fs::create_dir_all(&paths.root).expect("state directory");
        fs::write(&paths.config, "mispelled = 1\n").expect("write config");
        let error = ConfigStore::new(paths)
            .load()
            .expect_err("config should fail");
        assert_eq!(error.code, "invalid_config");
    }
}
