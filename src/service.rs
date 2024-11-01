use std::collections::HashMap;
use std::hash::Hash;
use std::net::Ipv4Addr;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::db::journal::RoundId;
use crate::db::tables::{self, ServiceId, SlotId, TeamId};
use crate::db::Db;
use crate::Registry;

pub type ServiceRegistry = Registry<ServiceId, Service>;
pub type SlotRegistry = Registry<SlotId, ServiceId>;
pub type ServiceStateMap = HashMap<ServiceStateKey, ServiceState>;

#[derive(Deserialize, Serialize, Clone, Hash, Debug, PartialEq, Eq)]
pub struct ServiceStateKey {
    pub round_id: RoundId,
    pub team_id: TeamId,
    pub service_id: ServiceId,
}

#[derive(Deserialize, Serialize, Clone, Hash, Debug, PartialEq, Eq)]
pub struct Service {
    name: String,
    port: u16,
    slot: u8,
    timeout: usize,
    concurrency_limit: usize,
    checker_host: Ipv4Addr,
    checker_port: u16,
}

impl Service {
    pub fn new(
        name: String,
        port: u16,
        slot: u8,
        timeout: usize,
        concurrency_limit: usize,
        checker_host: Ipv4Addr,
        checker_port: u16,
    ) -> Self {
        Self {
            name,
            port,
            slot,
            timeout,
            concurrency_limit,
            checker_host,
            checker_port,
        }
    }
    pub fn timeout(&self) -> usize {
        self.timeout
    }

    pub fn concurrency_limit(&self) -> usize {
        self.concurrency_limit
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn checker(&self) -> String {
        format!("http://{}:{}", self.checker_host, self.checker_port)
    }

    pub fn checker_host(&self) -> Ipv4Addr {
        self.checker_host
    }

    pub fn checker_port(&self) -> u16 {
        self.checker_port
    }

    pub fn slot(&self) -> u8 {
        self.slot
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum ServiceState {
    /// Service is up and working fine.
    Up,
    /// Unable to connect to the service.
    Down,
    /// A few of the valid flags from past rounds can't be retrieved.
    Recovering,
    /// Service is responding but some of the functionality is not working.
    Mumble(String),
    /// Unable to retrieve flag.
    Corrupt(String),
    /// Unable to identify the status of the service. Might need to retry.
    Unknown,
}

impl std::fmt::Display for ServiceState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", String::from(self))
    }
}

impl From<&ServiceState> for String {
    fn from(state: &ServiceState) -> Self {
        match state {
            ServiceState::Up => "Up".to_string(),
            ServiceState::Down => "Down".to_string(),
            ServiceState::Mumble(reason) => format!("Mumble({})", reason),
            ServiceState::Corrupt(reason) => format!("Corrupt({})", reason),
            ServiceState::Recovering => "Recovering".to_string(),
            ServiceState::Unknown => "Unknown".to_string(),
        }
    }
}

impl From<&ServiceState> for u8 {
    fn from(value: &ServiceState) -> Self {
        match value {
            ServiceState::Unknown => 1,
            ServiceState::Up => 2,
            ServiceState::Recovering => 3,
            ServiceState::Mumble(_) => 4,
            ServiceState::Corrupt(_) => 5,
            ServiceState::Down => 6,
        }
    }
}

impl From<(u8, &Option<String>)> for ServiceState {
    fn from(value: (u8, &Option<String>)) -> Self {
        match value.0 {
            1 => ServiceState::Unknown,
            2 => ServiceState::Up,
            3 => ServiceState::Recovering,
            4 => ServiceState::Mumble(value.1.clone().unwrap_or_default()),
            5 => ServiceState::Corrupt(value.1.clone().unwrap_or_default()),
            6 => ServiceState::Down,
            _ => ServiceState::Unknown,
        }
    }
}

impl ServiceState {
    pub fn to_num(&self) -> u8 {
        u8::from(self)
    }

    pub fn to_state(&self) -> &str {
        match self {
            Self::Unknown => "Unknown",
            Self::Up => "Up",
            Self::Down => "Down",
            Self::Mumble(_) => "Mumble",
            Self::Corrupt(_) => "Corrupt",
            Self::Recovering => "Recovering",
        }
    }

    pub fn reason(&self) -> Option<&String> {
        match self {
            Self::Mumble(reason) | Self::Corrupt(reason) => Some(reason),
            _ => None,
        }
    }
}

pub fn should_update<P, N>(previous_state: P, new_state: N) -> bool
where
    P: Into<u8>,
    N: Into<u8>,
{
    let previous_state: u8 = previous_state.into();
    let new_state: u8 = new_state.into();

    assert!(
        (ServiceState::Unknown.to_num()..=ServiceState::Down.to_num())
            .contains(&previous_state),
        "Invalid ServiceState Specified"
    );
    assert!(
        (ServiceState::Unknown.to_num()..=ServiceState::Down.to_num())
            .contains(&new_state),
        "Invalid ServiceState Specified"
    );

    // If the new state is Up, only update the state of the service if we are updating
    // from Unknown.
    if new_state == 2 {
        if previous_state == 1 || previous_state == 2 {
            return true;
        } else {
            return false;
        }
    } else {
        if previous_state <= new_state {
            return true;
        } else {
            return false;
        }
    }
}

pub fn commit_to_database<T: AsRef<std::path::Path>>(
    path: T,
    db: &Db,
) -> Result<ServiceRegistry> {
    #[derive(Deserialize)]
    struct Entry {
        name: String,
        port: u16,
        no_flag_slot: u8,
        checker_port: usize,
        checker_host: Ipv4Addr,

        timeout: Option<usize>,
        concurrency_limit: Option<usize>,
    }

    let services = serde_json::from_str::<Vec<Entry>>(
        &std::fs::read_to_string(path).context("Reading service json file")?,
    )
    .context("Deserializing service json file")?;

    // Validate service configs and create service vector. If the number of slots is greater
    // than one, we create multiple service structs with different slots so that we can
    // distinguish the service using ServiceId.
    for service in &services {
        if service.no_flag_slot == 0 {
            bail!(format!(
                "FlagSlot for service can't be zero: {}",
                service.name
            ));
        }

        // We set the default timeout and concurrency_limit to 60.
        let timeout = service.timeout.unwrap_or(60);
        let concurrency_limit = service.concurrency_limit.unwrap_or(60);

        // Once all the values are validated, insert the service details into the database
        tables::ServiceList::insert(
            db,
            service.name.to_owned(),
            service.port as usize,
            service.no_flag_slot as usize,
            timeout as usize,
            concurrency_limit as usize,
            service.checker_host.to_string(),
            service.checker_port as usize,
        )?;
    }

    Ok(tables::ServiceList::all(db)?)
}
pub fn log(registry: &ServiceRegistry) {
    // Define column widths
    let name_width = 15;
    let port_width = 7;
    let checker_width = 30;
    let slot_width = 7;
    let total_width = name_width + port_width + checker_width + slot_width + 5; // 5 for separators

    let horizontal_line = |c| format!("{}{}", c, "─".repeat(total_width - 1));

    info!("Loaded Services:");
    info!("{}", horizontal_line('┌'));
    info!(
        "│{:^name_width$}│{:^port_width$}│{:^checker_width$}│{:^slot_width$}│",
        "Name", "Port", "Checker Ip", "Slot",
    );
    info!("{}", horizontal_line('├'));
    for service in registry.values() {
        info!(
            "│{:^name_width$}│{:^port_width$}│{:^checker_width$}│{:^slot_width$}│",
            service.name(),
            service.port(),
            service.checker(),
            service.slot(),
        );
    }
    info!("{}", horizontal_line('└'));
}
