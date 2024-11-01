use derive_more::Deref;
use std::{net::IpAddr, path::Path};
use tracing::info;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{
    db::{
        tables::{self, TeamId},
        Db,
    },
    Registry,
};

pub type TeamRegistry = Registry<TeamId, Team>;

// Struct containing information about a particular team
#[derive(Deserialize, Serialize, Clone, Hash, Debug, Eq, PartialEq)]
pub struct Team {
    name: String,
    ip: IpAddr,
}

impl Team {
    pub fn new(ip: IpAddr, name: String) -> Self {
        Self { name, ip }
    }

    pub fn ip(&self) -> IpAddr {
        self.ip
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

#[derive(
    Debug, Clone, Copy, Hash, PartialEq, Eq, Deref, Serialize, Deserialize,
)]
#[repr(C)]
pub struct Attacker(TeamId);

impl Attacker {
    pub fn new(team_id: TeamId) -> Self {
        Self(team_id)
    }

    pub fn id(&self) -> TeamId {
        self.0
    }
}

#[derive(
    Debug, Clone, Copy, Hash, PartialEq, Eq, Deref, Serialize, Deserialize,
)]
#[repr(C)]
pub struct Victim(TeamId);
impl Victim {
    pub fn new(team_id: TeamId) -> Self {
        Self(team_id)
    }
}

pub fn commit_to_database<T: AsRef<Path>>(
    path: T,
    db: &Db,
) -> Result<TeamRegistry> {
    #[derive(Deserialize)]
    struct Entry {
        name: String,
        ip: IpAddr,
    }

    let entry = serde_json::from_str::<Vec<Entry>>(
        &std::fs::read_to_string(path).context("Reading teams json file")?,
    )
    .context("Deserializing teams json")?;

    for team in entry {
        // TODO: Validate data
        tables::TeamList::insert(db, team.name, team.ip.to_string())?;
    }
    let teams = tables::TeamList::all(db)?;
    Ok(teams)
}
pub fn log(registry: &TeamRegistry) {
    let name_width = 21;
    let ip_width = 21;
    let total_width = name_width + ip_width + 3; // 3 for separators

    info!("Loaded Teams:");
    info!("┌{}┐", "─".repeat(total_width - 2));
    info!("│{:^width$}│{:^width$}│", "Name", "IP", width = name_width);
    info!("├{}┼{}┤", "─".repeat(name_width), "─".repeat(ip_width));
    for team in registry.values() {
        info!(
            "│{:^width$}│{:^width$}│",
            team.name(),
            team.ip(),
            width = name_width
        );
    }
    info!("└{}┘", "─".repeat(total_width - 2));
}
