use serde::{Deserialize, Serialize};
use std::hash::Hash;

#[derive(Deserialize, Serialize, Debug, Hash, Clone)]
/// Represents the configuration for the application.
pub struct Config {
    name: String,
    flag_life: u64,
    tick_time: u64,
    flag_port: u16,
    total_round: u64,
}

impl Config {
    /// Creates a new `Config` instance with the given parameters.
    ///
    /// # Arguments
    ///
    /// * `name` - The name of the contest.
    /// * `flag_life` - The lifetime of a flag.
    /// * `tick_time` - The time between ticks.
    /// * `flag_port` - The port number for flag-related operations.
    /// * `total_round` - The total number of rounds.
    ///
    /// # Returns
    ///
    /// A new `Config` instance.
    pub fn new(
        name: String,
        flag_life: u64,
        tick_time: u64,
        flag_port: u16,
        total_round: u64,
    ) -> Self {
        Self {
            name,
            flag_life,
            tick_time,
            flag_port,
            total_round,
        }
    }

    /// Returns the name of the contest.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the lifetime of a flag.
    pub fn flag_life(&self) -> u64 {
        self.flag_life
    }

    /// Returns the time between ticks.
    pub fn tick_time(&self) -> u64 {
        self.tick_time
    }

    /// Returns the port number for flag-related operations.
    pub fn flag_port(&self) -> u16 {
        self.flag_port
    }

    /// Returns the total number of rounds.
    pub fn total_round(&self) -> u64 {
        self.total_round
    }
}

use log::info;

/// Logs the current game configuration in a table format.
#[rustfmt::skip]
pub fn log(config: &Config) {
    info!("Game Configuration:");
    info!("┌─────────────┬────────────────────┐");
    info!("│ Setting     │ Value              │");
    info!("├─────────────┼────────────────────┤");
    info!("│ Name        │ {:<18} │", config.name);
    info!("│ Flag Life   │ {:<18} │", format!("{} rounds", config.flag_life));
    info!("│ Tick Time   │ {:<18} │", format!("{} seconds", config.tick_time));
    info!("│ Flag Port   │ {:<18} │", config.flag_port);
    info!("│ Total Rounds│ {:<18} │", config.total_round);
    info!("└─────────────┴────────────────────┘");
}
