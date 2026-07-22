use std::collections::HashSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, UtcOffset};
use uuid::Uuid;

use crate::error::{PortaError, Result};

pub const REGISTRY_SCHEMA_VERSION: u32 = 2;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    pub schema_version: u32,
    pub maintenance: Maintenance,
    pub open_leases: Vec<OpenLease>,
    pub reservations: Vec<Reservation>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Maintenance {
    pub cleanup_trigger: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OpenLease {
    pub id: Uuid,
    pub port: u16,
    pub key: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RegistryV1 {
    pub schema_version: u32,
    pub maintenance: Maintenance,
    pub open_leases: Vec<OpenLeaseV1>,
    pub reservations: Vec<Reservation>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OpenLeaseV1 {
    pub id: Uuid,
    pub port: u16,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

impl From<RegistryV1> for Registry {
    fn from(registry: RegistryV1) -> Self {
        debug_assert_eq!(registry.schema_version, 1);
        Self {
            schema_version: REGISTRY_SCHEMA_VERSION,
            maintenance: registry.maintenance,
            open_leases: registry
                .open_leases
                .into_iter()
                .map(|lease| OpenLease {
                    id: lease.id,
                    port: lease.port,
                    key: None,
                    created_at: lease.created_at,
                    expires_at: lease.expires_at,
                })
                .collect(),
            reservations: registry.reservations,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Reservation {
    pub id: Uuid,
    pub directory: PathBuf,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub missing_since: Option<OffsetDateTime>,
    pub allocations: Vec<Allocation>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Allocation {
    pub port: u16,
    pub key: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

impl Registry {
    #[must_use]
    pub fn empty(cleanup_trigger: usize) -> Self {
        Self {
            schema_version: REGISTRY_SCHEMA_VERSION,
            maintenance: Maintenance { cleanup_trigger },
            open_leases: Vec::new(),
            reservations: Vec::new(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != REGISTRY_SCHEMA_VERSION {
            return Err(invalid_registry(format!(
                "Unsupported registry schema version: {}",
                self.schema_version
            )));
        }
        if !(1..=100).contains(&self.maintenance.cleanup_trigger) {
            return Err(invalid_registry(
                "maintenance.cleanup_trigger must be between 1 and 100",
            ));
        }

        let mut ids = HashSet::new();
        let mut ports = HashSet::new();
        let mut lease_keys = HashSet::new();
        for lease in &self.open_leases {
            if !ids.insert(lease.id) {
                return Err(invalid_registry("Registry IDs must be unique"));
            }
            if lease.port == 0 || !ports.insert(lease.port) {
                return Err(invalid_registry("Registry ports must be valid and unique"));
            }
            if let Some(key) = &lease.key
                && (key.is_empty() || !lease_keys.insert(key))
            {
                return Err(invalid_registry(
                    "Open lease keys must be non-empty and unique",
                ));
            }
            if lease.expires_at <= lease.created_at {
                return Err(invalid_registry("Lease expiration must follow creation"));
            }
            if lease.created_at.offset() != UtcOffset::UTC
                || lease.expires_at.offset() != UtcOffset::UTC
            {
                return Err(invalid_registry("Registry timestamps must use UTC"));
            }
        }

        let mut directories = HashSet::new();
        for reservation in &self.reservations {
            if !ids.insert(reservation.id) {
                return Err(invalid_registry("Registry IDs must be unique"));
            }
            if !reservation.directory.is_absolute()
                || reservation.directory.to_str().is_none()
                || !directories.insert(reservation.directory.clone())
            {
                return Err(invalid_registry(
                    "Reservation directories must be absolute, UTF-8, and unique",
                ));
            }
            if reservation.allocations.is_empty() {
                return Err(invalid_registry("A reservation must contain an allocation"));
            }
            if reservation
                .expires_at
                .is_some_and(|expires_at| expires_at <= reservation.created_at)
            {
                return Err(invalid_registry(
                    "Reservation expiration must follow creation",
                ));
            }
            if reservation.created_at.offset() != UtcOffset::UTC
                || reservation
                    .expires_at
                    .is_some_and(|value| value.offset() != UtcOffset::UTC)
                || reservation
                    .missing_since
                    .is_some_and(|value| value.offset() != UtcOffset::UTC)
            {
                return Err(invalid_registry("Registry timestamps must use UTC"));
            }

            let mut keys = HashSet::new();
            for allocation in &reservation.allocations {
                if allocation.port == 0 || !ports.insert(allocation.port) {
                    return Err(invalid_registry("Registry ports must be valid and unique"));
                }
                if let Some(key) = &allocation.key
                    && (key.is_empty() || !keys.insert(key))
                {
                    return Err(invalid_registry(
                        "Reservation keys must be non-empty and unique",
                    ));
                }
                if allocation.created_at.offset() != UtcOffset::UTC {
                    return Err(invalid_registry("Registry timestamps must use UTC"));
                }
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn used_ports(&self) -> HashSet<u16> {
        self.open_leases
            .iter()
            .map(|lease| lease.port)
            .chain(
                self.reservations
                    .iter()
                    .flat_map(|reservation| reservation.allocations.iter().map(|item| item.port)),
            )
            .collect()
    }
}

fn invalid_registry(message: impl Into<String>) -> PortaError {
    PortaError::invalid("invalid_registry", message)
}

#[cfg(test)]
mod tests {
    use time::{Duration, OffsetDateTime};
    use uuid::Uuid;

    use super::{OpenLease, Registry};

    #[test]
    fn rejects_duplicate_ports() {
        let now = OffsetDateTime::now_utc();
        let mut registry = Registry::empty(30);
        registry.open_leases = vec![
            OpenLease {
                id: Uuid::new_v4(),
                port: 55_000,
                key: None,
                created_at: now,
                expires_at: now + Duration::minutes(2),
            },
            OpenLease {
                id: Uuid::new_v4(),
                port: 55_000,
                key: None,
                created_at: now,
                expires_at: now + Duration::minutes(2),
            },
        ];
        assert!(registry.validate().is_err());
    }

    #[test]
    fn rejects_duplicate_open_lease_keys() {
        let now = OffsetDateTime::now_utc();
        let mut registry = Registry::empty(30);
        registry.open_leases = vec![
            OpenLease {
                id: Uuid::new_v4(),
                port: 55_000,
                key: Some("preview".to_owned()),
                created_at: now,
                expires_at: now + Duration::minutes(2),
            },
            OpenLease {
                id: Uuid::new_v4(),
                port: 55_001,
                key: Some("preview".to_owned()),
                created_at: now,
                expires_at: now + Duration::minutes(2),
            },
        ];
        assert!(registry.validate().is_err());
    }
}
