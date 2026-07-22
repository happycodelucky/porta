use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Serialize;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::config::{Config, ConfigEntry, ConfigStore, ConfigValue};
use crate::duration::{parse_duration, parse_expiration};
use crate::error::{PortaError, Result};
use crate::registry::{
    Allocation, JsonRegistryStore, OpenLease, Registry, RegistryStore, RegistryTransaction,
    Reservation,
};
use crate::socket::{port_is_bound, select_ports};
use crate::state::{StatePaths, resolve_directory};

const CLEANUP_TRIGGER_CAP: usize = 100;
const CLEANUP_LOW_YIELD_MAX: usize = 10;

pub struct PortaService {
    config: ConfigStore,
    registry: JsonRegistryStore,
}

#[derive(Clone, Debug)]
pub struct ReserveRequest {
    pub directory: PathBuf,
    pub base: Option<u16>,
    pub keys: Vec<String>,
    pub contiguous: bool,
    pub expires: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LeaseResult {
    pub port: u16,
    pub key: Option<String>,
    pub lease_id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReserveResult {
    pub directory: PathBuf,
    pub reservation_id: Uuid,
    #[serde(with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
    pub ports: Vec<u16>,
    pub allocations: Vec<Allocation>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub keyed_ports: BTreeMap<String, u16>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reused_keys: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReleaseResult {
    pub directory: PathBuf,
    pub ports: Vec<u16>,
    pub released: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReleasePortsResult {
    pub ports: Vec<u16>,
    pub released: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct GetResult {
    pub directory: PathBuf,
    pub port: u16,
    pub key: Option<String>,
    pub allocation: Allocation,
}

#[derive(Clone, Debug, Serialize)]
pub struct PortInfo {
    pub port: u16,
    pub state: String,
    pub in_use: bool,
    pub key: Option<String>,
    pub directory: Option<PathBuf>,
    pub lease_id: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReservationView {
    pub id: Uuid,
    pub directory: PathBuf,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub missing_since: Option<OffsetDateTime>,
    pub status: String,
    pub ports: Vec<u16>,
    pub keyed_ports: BTreeMap<String, u16>,
    pub allocations: Vec<Allocation>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LeaseView {
    pub id: Uuid,
    pub port: u16,
    pub key: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    pub status: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ListResult {
    pub reservations: Vec<ReservationView>,
    pub leases: Vec<LeaseView>,
    #[serde(skip)]
    pub in_use: BTreeMap<u16, bool>,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct CleanResult {
    pub released_leases: usize,
    pub expired_reservations: usize,
    pub marked_missing: usize,
    pub restored: usize,
    pub reaped: usize,
    pub cleanup_trigger: usize,
}

impl PortaService {
    pub fn discover() -> Result<Self> {
        Ok(Self::new(StatePaths::discover()?))
    }

    #[must_use]
    pub fn new(paths: StatePaths) -> Self {
        Self {
            config: ConfigStore::new(paths.clone()),
            registry: JsonRegistryStore::new(paths),
        }
    }

    pub fn config_get(&self, key: &str) -> Result<ConfigValue> {
        self.config.get(key)
    }

    pub fn config_list(&self) -> Result<Vec<ConfigEntry>> {
        self.config.list()
    }

    pub fn config_set(&self, key: &str, value: &str) -> Result<ConfigValue> {
        self.config.set(key, value)
    }

    pub fn lease(
        &self,
        base: Option<u16>,
        timeout: Option<&str>,
        key: Option<&str>,
    ) -> Result<LeaseResult> {
        if key.is_some_and(str::is_empty) {
            return Err(PortaError::invalid(
                "invalid_arguments",
                "Keys cannot be empty",
            ));
        }
        let config = self.config.load()?;
        let base = validate_base(base.unwrap_or(config.default_port))?;
        let timeout = timeout.map_or(Ok(config.lease_timeout), |value| {
            parse_duration(value, true, false)
        })?;
        let now = OffsetDateTime::now_utc();
        let expires_at = now
            .checked_add(
                time::Duration::try_from(timeout)
                    .map_err(|_| PortaError::invalid("invalid_duration", "timeout is too large"))?,
            )
            .ok_or_else(|| PortaError::invalid("invalid_duration", "timeout is too large"))?;

        let mut transaction = self.registry.begin(config.cleanup_trigger_start)?;
        sweep_expired(&mut transaction, now)?;
        automatic_cleanup(&mut transaction, &config, now)?;
        if let Some(key) = key
            && let Some(index) = transaction
                .registry()
                .open_leases
                .iter()
                .position(|lease| lease.key.as_deref() == Some(key))
        {
            let lease = {
                let lease = &mut transaction.registry_mut().open_leases[index];
                lease.expires_at = expires_at;
                lease.clone()
            };
            transaction.commit()?;
            return Ok(lease_result(&lease));
        }
        let registered = transaction.registry().used_ports();
        let selection = select_ports(base, 1, false, &registered)?;
        let port = selection.ports[0];
        let lease = OpenLease {
            id: Uuid::new_v4(),
            port,
            key: key.map(str::to_owned),
            created_at: now,
            expires_at,
        };
        transaction.registry_mut().open_leases.push(lease.clone());
        transaction.commit()?;
        drop(selection);
        Ok(lease_result(&lease))
    }

    #[allow(clippy::too_many_lines)]
    pub fn reserve(&self, request: &ReserveRequest) -> Result<ReserveResult> {
        validate_keys(&request.keys)?;
        let directory = resolve_directory(&request.directory, true)?;
        let config = self.config.load()?;
        let base = validate_base(request.base.unwrap_or(config.default_port))?;
        let now = OffsetDateTime::now_utc();
        let requested_expiration = request
            .expires
            .as_deref()
            .map(|value| parse_expiration(value, now))
            .transpose()?;

        let mut transaction = self.registry.begin(config.cleanup_trigger_start)?;
        sweep_expired(&mut transaction, now)?;
        automatic_cleanup(&mut transaction, &config, now)?;

        let existing_index = transaction
            .registry()
            .reservations
            .iter()
            .position(|reservation| reservation.directory == directory);
        let existing_by_key: HashMap<String, Allocation> = existing_index
            .map(|index| {
                transaction.registry().reservations[index]
                    .allocations
                    .iter()
                    .filter_map(|allocation| {
                        allocation
                            .key
                            .as_ref()
                            .map(|key| (key.clone(), allocation.clone()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let reused_keys: Vec<String> = request
            .keys
            .iter()
            .filter(|key| existing_by_key.contains_key(*key))
            .cloned()
            .collect();
        let new_keys: Vec<String> = request
            .keys
            .iter()
            .filter(|key| !existing_by_key.contains_key(*key))
            .cloned()
            .collect();
        let allocation_count = if request.keys.is_empty() {
            1
        } else {
            new_keys.len()
        };
        let registered = transaction.registry().used_ports();
        let selection = if allocation_count == 0 {
            None
        } else {
            Some(select_ports(
                base,
                allocation_count,
                request.contiguous,
                &registered,
            )?)
        };

        let index = if let Some(index) = existing_index {
            index
        } else {
            transaction.registry_mut().reservations.push(Reservation {
                id: Uuid::new_v4(),
                directory: directory.clone(),
                created_at: now,
                expires_at: requested_expiration,
                missing_since: None,
                allocations: Vec::new(),
            });
            transaction.registry().reservations.len() - 1
        };

        let reservation = &mut transaction.registry_mut().reservations[index];
        reservation.missing_since = None;
        if request.expires.is_some() {
            reservation.expires_at = requested_expiration;
        }
        let new_allocations: Vec<Allocation> = selection
            .as_ref()
            .map(|selected| {
                selected
                    .ports
                    .iter()
                    .enumerate()
                    .map(|(position, port)| Allocation {
                        port: *port,
                        key: if request.keys.is_empty() {
                            None
                        } else {
                            Some(new_keys[position].clone())
                        },
                        created_at: now,
                    })
                    .collect()
            })
            .unwrap_or_default();
        reservation.allocations.extend(new_allocations.clone());

        let allocations = if request.keys.is_empty() {
            new_allocations
        } else {
            let by_key: HashMap<&str, &Allocation> = reservation
                .allocations
                .iter()
                .filter_map(|allocation| allocation.key.as_deref().map(|key| (key, allocation)))
                .collect();
            request
                .keys
                .iter()
                .map(|key| by_key[key.as_str()].clone())
                .collect()
        };
        let result = reserve_result(reservation, allocations, reused_keys);
        transaction.commit()?;
        drop(selection);
        Ok(result)
    }

    pub fn release(&self, directory: &Path, keys: &[String]) -> Result<ReleaseResult> {
        validate_keys(keys)?;
        let directory = resolve_directory(directory, false)?;
        let config = self.config.load()?;
        let now = OffsetDateTime::now_utc();
        let mut transaction = self.registry.begin(config.cleanup_trigger_start)?;
        sweep_expired(&mut transaction, now)?;
        let index = reservation_index(transaction.registry(), &directory)?;

        let released = if keys.is_empty() {
            transaction.registry().reservations[index]
                .allocations
                .clone()
        } else {
            let by_key: HashMap<&str, &Allocation> = transaction.registry().reservations[index]
                .allocations
                .iter()
                .filter_map(|allocation| allocation.key.as_deref().map(|key| (key, allocation)))
                .collect();
            let missing: Vec<&String> = keys
                .iter()
                .filter(|key| !by_key.contains_key(key.as_str()))
                .collect();
            if !missing.is_empty() {
                return Err(PortaError::missing(
                    "key_not_found",
                    format!(
                        "No port is reserved for key(s): {}",
                        missing
                            .iter()
                            .map(|key| key.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ));
            }
            keys.iter()
                .map(|key| (*by_key[key.as_str()]).clone())
                .collect()
        };
        let ports: HashSet<u16> = released.iter().map(|allocation| allocation.port).collect();
        if keys.is_empty() {
            transaction.registry_mut().reservations.remove(index);
        } else {
            let reservation = &mut transaction.registry_mut().reservations[index];
            reservation
                .allocations
                .retain(|allocation| !ports.contains(&allocation.port));
            if reservation.allocations.is_empty() {
                transaction.registry_mut().reservations.remove(index);
            }
        }
        transaction.commit()?;
        let mut ports: Vec<u16> = ports.into_iter().collect();
        ports.sort_unstable();
        Ok(ReleaseResult {
            directory,
            released: ports.len(),
            ports,
        })
    }

    pub fn release_ports(&self, ports: &[u16]) -> Result<ReleasePortsResult> {
        if ports.is_empty() {
            return Err(PortaError::invalid(
                "invalid_arguments",
                "At least one port is required",
            ));
        }
        if ports.contains(&0) {
            return Err(PortaError::invalid(
                "invalid_port",
                "Port must be between 1 and 65535",
            ));
        }
        let requested: HashSet<u16> = ports.iter().copied().collect();
        if requested.len() != ports.len() {
            return Err(PortaError::invalid(
                "duplicate_port",
                "The same port cannot be requested more than once",
            ));
        }

        let config = self.config.load()?;
        let now = OffsetDateTime::now_utc();
        let mut transaction = self.registry.begin(config.cleanup_trigger_start)?;
        sweep_expired(&mut transaction, now)?;
        let registered = transaction.registry().used_ports();
        let mut missing: Vec<u16> = requested.difference(&registered).copied().collect();
        if !missing.is_empty() {
            missing.sort_unstable();
            return Err(PortaError::missing(
                "port_not_found",
                format!(
                    "No reservation or lease exists for port(s): {}",
                    missing
                        .iter()
                        .map(u16::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }

        let registry = transaction.registry_mut();
        registry
            .open_leases
            .retain(|lease| !requested.contains(&lease.port));
        for reservation in &mut registry.reservations {
            reservation
                .allocations
                .retain(|allocation| !requested.contains(&allocation.port));
        }
        registry
            .reservations
            .retain(|reservation| !reservation.allocations.is_empty());
        transaction.commit()?;

        let mut ports: Vec<u16> = requested.into_iter().collect();
        ports.sort_unstable();
        Ok(ReleasePortsResult {
            released: ports.len(),
            ports,
        })
    }

    pub fn get(&self, directory: &Path, key: Option<&str>) -> Result<GetResult> {
        let directory = resolve_directory(directory, false)?;
        let config = self.config.load()?;
        let now = OffsetDateTime::now_utc();
        let mut transaction = self.registry.begin(config.cleanup_trigger_start)?;
        sweep_expired(&mut transaction, now)?;
        let index = reservation_index(transaction.registry(), &directory)?;
        let reservation = &transaction.registry().reservations[index];
        let allocation = if let Some(key) = key {
            reservation
                .allocations
                .iter()
                .find(|allocation| allocation.key.as_deref() == Some(key))
                .cloned()
                .ok_or_else(|| {
                    PortaError::missing(
                        "key_not_found",
                        format!("No port is reserved for key: {key}"),
                    )
                })?
        } else if reservation.allocations.len() == 1 {
            reservation.allocations[0].clone()
        } else {
            return Err(PortaError::missing(
                "key_required",
                "A key is required when a reservation has multiple ports",
            ));
        };
        transaction.commit()?;
        Ok(GetResult {
            directory,
            port: allocation.port,
            key: allocation.key.clone(),
            allocation,
        })
    }

    pub fn info(&self, ports: &[u16]) -> Result<Vec<PortInfo>> {
        if ports.contains(&0) {
            return Err(PortaError::invalid(
                "invalid_port",
                "Port must be between 1 and 65535",
            ));
        }
        let config = self.config.load()?;
        let now = OffsetDateTime::now_utc();
        let mut transaction = self.registry.begin(config.cleanup_trigger_start)?;
        sweep_expired(&mut transaction, now)?;
        let mut results = Vec::with_capacity(ports.len());
        for port in ports {
            results.push(port_info(transaction.registry(), *port, now)?);
        }
        transaction.commit()?;
        Ok(results)
    }

    pub fn list(&self) -> Result<ListResult> {
        let config = self.config.load()?;
        let now = OffsetDateTime::now_utc();
        let mut transaction = self.registry.begin(config.cleanup_trigger_start)?;
        sweep_expired(&mut transaction, now)?;
        let mut reservations = transaction.registry().reservations.clone();
        reservations.sort_by(|left, right| left.directory.cmp(&right.directory));
        let views: Vec<ReservationView> = reservations
            .iter_mut()
            .map(|reservation| {
                reservation
                    .allocations
                    .sort_by_key(|allocation| allocation.port);
                reservation_view(reservation)
            })
            .collect();
        let mut open_leases = transaction.registry().open_leases.clone();
        open_leases.sort_by_key(|lease| lease.port);
        let leases: Vec<LeaseView> = open_leases
            .iter()
            .map(|lease| lease_view(lease, now))
            .collect();

        let mut in_use = BTreeMap::new();
        for reservation in &reservations {
            for allocation in &reservation.allocations {
                in_use.insert(allocation.port, port_is_bound(allocation.port)?);
            }
        }
        for lease in &open_leases {
            in_use.insert(lease.port, port_is_bound(lease.port)?);
        }
        transaction.commit()?;
        Ok(ListResult {
            reservations: views,
            leases,
            in_use,
        })
    }

    pub fn clean(&self) -> Result<CleanResult> {
        let config = self.config.load()?;
        let now = OffsetDateTime::now_utc();
        let mut transaction = self.registry.begin(config.cleanup_trigger_start)?;
        let expiration = sweep_expired(&mut transaction, now)?;
        let missing = observe_missing(transaction.registry_mut(), &config, now)?;
        let cleanup_trigger = transaction.registry().maintenance.cleanup_trigger;
        transaction.commit()?;
        Ok(CleanResult {
            released_leases: expiration.released_leases,
            expired_reservations: expiration.expired_reservations,
            marked_missing: missing.marked_missing,
            restored: missing.restored,
            reaped: missing.reaped,
            cleanup_trigger,
        })
    }
}

fn reserve_result(
    reservation: &Reservation,
    allocations: Vec<Allocation>,
    reused_keys: Vec<String>,
) -> ReserveResult {
    let ports = allocations
        .iter()
        .map(|allocation| allocation.port)
        .collect();
    let keyed_ports = allocations
        .iter()
        .filter_map(|allocation| {
            allocation
                .key
                .as_ref()
                .map(|key| (key.clone(), allocation.port))
        })
        .collect();
    ReserveResult {
        directory: reservation.directory.clone(),
        reservation_id: reservation.id,
        expires_at: reservation.expires_at,
        ports,
        allocations,
        keyed_ports,
        reused_keys,
    }
}

fn reservation_index(registry: &Registry, directory: &Path) -> Result<usize> {
    registry
        .reservations
        .iter()
        .position(|reservation| reservation.directory == directory)
        .ok_or_else(|| {
            PortaError::missing(
                "reservation_not_found",
                format!("No reservation exists for {}", directory.display()),
            )
        })
}

fn validate_base(base: u16) -> Result<u16> {
    if base == 0 {
        return Err(PortaError::invalid(
            "invalid_port",
            "Port must be between 1 and 65535",
        ));
    }
    Ok(base)
}

fn validate_keys(keys: &[String]) -> Result<()> {
    if keys.iter().any(String::is_empty) {
        return Err(PortaError::invalid(
            "invalid_arguments",
            "Keys cannot be empty",
        ));
    }
    let unique: HashSet<&String> = keys.iter().collect();
    if unique.len() != keys.len() {
        return Err(PortaError::invalid(
            "duplicate_key",
            "The same key cannot be requested more than once",
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default)]
struct ExpirationStats {
    released_leases: usize,
    expired_reservations: usize,
}

fn sweep_expired(
    transaction: &mut RegistryTransaction,
    now: OffsetDateTime,
) -> Result<ExpirationStats> {
    let registry = transaction.registry_mut();
    let leases = std::mem::take(&mut registry.open_leases);
    let mut retained = Vec::with_capacity(leases.len());
    let mut released_leases = 0;
    for lease in leases {
        if lease.expires_at > now || port_is_bound(lease.port)? {
            retained.push(lease);
        } else {
            released_leases += 1;
        }
    }
    registry.open_leases = retained;
    let before = registry.reservations.len();
    registry.reservations.retain(|reservation| {
        reservation
            .expires_at
            .is_none_or(|expires_at| expires_at > now)
    });
    Ok(ExpirationStats {
        released_leases,
        expired_reservations: before - registry.reservations.len(),
    })
}

#[derive(Clone, Copy, Debug, Default)]
struct MissingStats {
    marked_missing: usize,
    restored: usize,
    reaped: usize,
}

fn observe_missing(
    registry: &mut Registry,
    config: &Config,
    now: OffsetDateTime,
) -> Result<MissingStats> {
    let grace = time::Duration::try_from(config.missing_for)
        .map_err(|_| PortaError::invalid("invalid_config", "missing_for duration is too large"))?;
    let mut stats = MissingStats::default();
    let reservations = std::mem::take(&mut registry.reservations);
    for mut reservation in reservations {
        if reservation.directory.is_dir() {
            if reservation.missing_since.take().is_some() {
                stats.restored += 1;
            }
            registry.reservations.push(reservation);
        } else if let Some(missing_since) = reservation.missing_since {
            if now - missing_since >= grace {
                stats.reaped += 1;
            } else {
                registry.reservations.push(reservation);
            }
        } else {
            reservation.missing_since = Some(now);
            stats.marked_missing += 1;
            registry.reservations.push(reservation);
        }
    }
    Ok(stats)
}

fn automatic_cleanup(
    transaction: &mut RegistryTransaction,
    config: &Config,
    now: OffsetDateTime,
) -> Result<()> {
    if transaction.registry().reservations.len()
        < transaction.registry().maintenance.cleanup_trigger
    {
        return Ok(());
    }
    let stats = observe_missing(transaction.registry_mut(), config, now)?;
    if stats.reaped <= CLEANUP_LOW_YIELD_MAX {
        let trigger = transaction.registry().maintenance.cleanup_trigger;
        transaction.registry_mut().maintenance.cleanup_trigger = trigger
            .saturating_add(config.cleanup_trigger_step)
            .min(CLEANUP_TRIGGER_CAP);
    }
    Ok(())
}

fn port_info(registry: &Registry, port: u16, now: OffsetDateTime) -> Result<PortInfo> {
    for reservation in &registry.reservations {
        if let Some(allocation) = reservation
            .allocations
            .iter()
            .find(|allocation| allocation.port == port)
        {
            return Ok(PortInfo {
                port,
                state: "reserved".to_owned(),
                in_use: port_is_bound(port)?,
                key: allocation.key.clone(),
                directory: Some(reservation.directory.clone()),
                lease_id: None,
                expires_at: reservation.expires_at,
            });
        }
    }
    if let Some(lease) = registry.open_leases.iter().find(|lease| lease.port == port) {
        let in_use = port_is_bound(port)?;
        return Ok(PortInfo {
            port,
            state: if lease.expires_at <= now {
                "expired_in_use".to_owned()
            } else {
                "leased".to_owned()
            },
            in_use,
            key: lease.key.clone(),
            directory: None,
            lease_id: Some(lease.id),
            expires_at: Some(lease.expires_at),
        });
    }
    let in_use = port_is_bound(port)?;
    Ok(PortInfo {
        port,
        state: if in_use { "in_use" } else { "available" }.to_owned(),
        in_use,
        key: None,
        directory: None,
        lease_id: None,
        expires_at: None,
    })
}

fn reservation_view(reservation: &Reservation) -> ReservationView {
    let ports = reservation
        .allocations
        .iter()
        .map(|allocation| allocation.port)
        .collect();
    let keyed_ports = reservation
        .allocations
        .iter()
        .filter_map(|allocation| {
            allocation
                .key
                .as_ref()
                .map(|key| (key.clone(), allocation.port))
        })
        .collect();
    ReservationView {
        id: reservation.id,
        directory: reservation.directory.clone(),
        created_at: reservation.created_at,
        expires_at: reservation.expires_at,
        missing_since: reservation.missing_since,
        status: if reservation.directory.is_dir() {
            "active".to_owned()
        } else {
            "missing".to_owned()
        },
        ports,
        keyed_ports,
        allocations: reservation.allocations.clone(),
    }
}

fn lease_view(lease: &OpenLease, now: OffsetDateTime) -> LeaseView {
    LeaseView {
        id: lease.id,
        port: lease.port,
        key: lease.key.clone(),
        created_at: lease.created_at,
        expires_at: lease.expires_at,
        status: if lease.expires_at <= now {
            "expired_in_use".to_owned()
        } else {
            "leased".to_owned()
        },
    }
}

fn lease_result(lease: &OpenLease) -> LeaseResult {
    LeaseResult {
        port: lease.port,
        key: lease.key.clone(),
        lease_id: lease.id,
        created_at: lease.created_at,
        expires_at: lease.expires_at,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{PortaService, ReserveRequest};
    use crate::state::StatePaths;

    #[test]
    fn keyed_reservation_is_additive_and_idempotent() {
        let temporary = tempdir().expect("temporary directory");
        let workspace = temporary.path().join("workspace");
        fs::create_dir(&workspace).expect("workspace");
        let service = PortaService::new(StatePaths::from_root(temporary.path().join("state")));
        let first = service
            .reserve(&ReserveRequest {
                directory: workspace.clone(),
                base: Some(55_000),
                keys: vec!["web".to_owned()],
                contiguous: false,
                expires: None,
            })
            .expect("first reservation");
        let second = service
            .reserve(&ReserveRequest {
                directory: workspace,
                base: Some(55_000),
                keys: vec!["web".to_owned(), "api".to_owned()],
                contiguous: false,
                expires: None,
            })
            .expect("second reservation");
        assert_eq!(first.ports[0], second.ports[0]);
        assert_eq!(second.reused_keys, vec!["web"]);
        assert_ne!(second.ports[0], second.ports[1]);
    }
}
