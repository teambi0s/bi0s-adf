use std::{
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
};

use anyhow::{Context, Result};

pub mod tables {
    use std::collections::HashMap;

    use anyhow::Context;
    use derive_more::Deref;
    use rusqlite::params;
    use serde::{Deserialize, Serialize};

    use crate::flag::FlagIdentifier;
    use crate::flag::FlagRegistry;
    use crate::flag::FlagSecret;
    use crate::flag::Token;
    use crate::service::Service;
    use crate::service::ServiceRegistry;
    use crate::service::SlotRegistry;
    use crate::{
        flag::Flag,
        teams::{Team, TeamRegistry},
    };

    use super::journal::RoundId;
    use super::Db;

    pub fn initialize_database(db: &Db) -> anyhow::Result<()> {
        let conn = db.inner()?;
        // Create a table for storing details about all the services
        conn.execute(
            "CREATE TABLE IF NOT EXISTS services (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                port INTEGER NOT NULL,
                slot INTEGER NOT NULL,
                timeout INTEGER NOT NULL,
                concurrency_limit INTEGER NOT NULL,
                checker_host TEXT NOT NULL,
                checker_port INTEGER NOT NULL
            )",
            [],
        )
        .context("Failed to create services table")?;

        // Create a table for storing slots for services
        conn.execute(
            "CREATE TABLE IF NOT EXISTS service_slots (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                service_id INTEGER NOT NULL,
                slot_number INTEGER NOT NULL,
                FOREIGN KEY (service_id) REFERENCES services(id),
                UNIQUE (service_id, slot_number)
            )",
            [],
        )
        .context("Failed to create service_slots table")?;

        // Create a table for teams
        conn.execute(
            "CREATE TABLE IF NOT EXISTS teams (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                ip TEXT NOT NULL
            )",
            [],
        )
        .context("Failed to create teams table")?;

        // Create a table for storing config
        conn.execute(
            "CREATE TABLE IF NOT EXISTS config (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                name TEXT NOT NULL,
                flag_life INTEGER NOT NULL,
                tick_time INTEGER NOT NULL,
                flag_port INTEGER NOT NULL,
                total_round INTEGER NOT NULL
            )",
            [],
        )
        .context("Failed to create config table")?;

        // Create a table for storing flags
        conn.execute(
            "CREATE TABLE IF NOT EXISTS flags (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                round_id INTEGER NOT NULL,
                slot INTEGER NOT NULL,
                team_id INTEGER NOT NULL,
                service_id INTEGER NOT NULL,
                secret BLOB NOT NULL,
                token TEXT,
                identifier TEXT,
                FOREIGN KEY (round_id) REFERENCES rounds(id),
                FOREIGN KEY (service_id) REFERENCES services(id)
            )",
            [],
        )
        .context("Failed to create flags table")?;

        // Insert the default round with round_number 0 and finished set to true
        conn.execute(
            "INSERT INTO rounds (start_time, round_number, finished) VALUES (?, ?, 1)",
            params![chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default(), 0],
        )
        .context("Failed to insert default round")?;

        Ok(())
    }

    pub struct ServiceStateStore;

    pub struct FlagStore;

    #[derive(
        Clone, Copy, Serialize, Deserialize, Debug, PartialEq, Eq, Hash, Deref,
    )]
    pub struct FlagId(pub(super) i64);

    impl FlagStore {
        pub fn insert(db: &Db, flag: &Flag) -> anyhow::Result<FlagId> {
            let conn = db.inner()?;
            let mut stmt = conn.prepare(
                "INSERT INTO flags (round_id, slot, team_id, service_id, secret, token, identifier)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;

            let flag_id = stmt.insert(params![
                flag.round_id().id,
                flag.slot().get_id(),
                flag.team_id().0,
                flag.service_id().0,
                flag.secret().encode(),
                flag.token().to_string(),
                flag.identifier().to_string(),
            ])?;

            Ok(FlagId(flag_id))
        }

        pub fn get(db: &Db, flag_id: FlagId) -> anyhow::Result<Flag> {
            let conn = db.inner()?;
            let mut stmt = conn.prepare(
                "SELECT f.round_id, s.id, s.slot_number, f.team_id, f.service_id, f.secret, f.token, f.identifier, r.round_number 
                 FROM flags f 
                 JOIN rounds r ON f.round_id = r.id 
                 JOIN service_slots s ON f.slot = s.id 
                 WHERE f.id = ?",
            )?;

            let flag = stmt.query_row([flag_id.0], |row| {
                let round_id = RoundId {
                    id: row.get(0)?,
                    round_number: row.get(8)?,
                };
                let slot = SlotId {
                    id: row.get(1)?,
                    slot_number: row.get(2)?,
                };
                let team_id = TeamId(row.get(3)?);
                let service_id = ServiceId(row.get(4)?);
                let secret_hex: String = row.get(5)?;
                let secret =
                    FlagSecret::decode(&secret_hex).unwrap_or_default();

                let token: Option<String> = row.get(6)?;
                let identifier: Option<String> = row.get(7)?;

                let flag = Flag::import(
                    round_id,
                    slot,
                    team_id,
                    service_id,
                    secret,
                    Token::new(token),
                    FlagIdentifier::new(identifier),
                );
                Ok(flag)
            })?;

            if flag.secret() == &FlagSecret::default() {
                Err(anyhow::anyhow!("Unable to decode FlagSecret"))
            } else {
                Ok(flag)
            }
        }

        pub fn get_by_round(
            db: &Db,
            round_id: RoundId,
        ) -> anyhow::Result<Vec<(FlagId, Flag)>> {
            let conn = db.inner()?;
            let mut stmt = conn.prepare(
                "SELECT f.id, s.id, s.slot_number, f.team_id, f.service_id, f.secret, f.token, f.identifier 
                 FROM flags f 
                 JOIN service_slots s ON f.slot = s.id 
                 WHERE f.round_id = ?",
            )?;

            let flags = stmt
                .query_map([round_id.id], |row| {
                    let flag_id = FlagId(row.get(0)?);
                    let slot_id = row.get(1)?;
                    let slot_number = row.get(2)?;
                    let slot = SlotId {
                        id: slot_id,
                        slot_number,
                    };
                    let team_id = TeamId(row.get(3)?);
                    let service_id = ServiceId(row.get(4)?);
                    let secret_hex: String = row.get(5)?;
                    let secret =
                        FlagSecret::decode(&secret_hex).unwrap_or_default();

                    let token: Option<String> = {
                        let token: String = row.get(6)?;
                        if token.is_empty() {
                            None
                        } else {
                            Some(token)
                        }
                    };
                    let identifier: Option<String> = {
                        let identifier: String = row.get(7)?;
                        if identifier.is_empty() {
                            None
                        } else {
                            Some(identifier)
                        }
                    };

                    let flag = Flag::import(
                        round_id,
                        slot,
                        team_id,
                        service_id,
                        secret,
                        Token::new(token),
                        FlagIdentifier::new(identifier),
                    );

                    Ok((flag_id, flag))
                })?
                .collect::<Result<Vec<_>, _>>()?;

            Ok(flags)
        }

        pub fn update_token(
            db: &Db,
            flag_id: FlagId,
            new_token: Token,
        ) -> anyhow::Result<()> {
            let conn = db.inner()?;
            conn.execute(
                "UPDATE flags SET token = ?1 WHERE id = ?2",
                params![new_token.to_string(), flag_id.0],
            )?;
            Ok(())
        }

        pub fn update_identifier(
            db: &Db,
            flag_id: FlagId,
            new_identifier: FlagIdentifier,
        ) -> anyhow::Result<()> {
            let conn = db.inner()?;
            conn.execute(
                "UPDATE flags SET identifier = ?1 WHERE id = ?2",
                params![new_identifier.to_string(), flag_id.0],
            )?;
            Ok(())
        }

        pub fn all(db: &Db) -> anyhow::Result<FlagRegistry> {
            let conn = db.inner()?;
            let mut stmt = conn.prepare(
                "SELECT f.id, f.round_id, f.slot, f.team_id, f.service_id, f.secret, f.token, f.identifier, r.round_number, s.id, s.slot_number 
                 FROM flags f 
                 JOIN rounds r ON f.round_id = r.id 
                 JOIN service_slots s ON f.slot = s.id
                 WHERE r.finished = 1",
            )?;

            let flags = stmt
                .query_map([], |row| {
                    let flag_id = FlagId(row.get(0)?);
                    let round_id = RoundId {
                        id: row.get(1)?,
                        round_number: row.get(8)?,
                    };
                    let id = row.get(9)?;
                    let slot_number: i64 = row.get(10)?;
                    let slot = SlotId { slot_number, id };
                    let team_id = TeamId(row.get(3)?);
                    let service_id = ServiceId(row.get(4)?);
                    let secret_hex: String = row.get(5)?;
                    let secret =
                        FlagSecret::decode(&secret_hex).unwrap_or_default();

                    let token: Option<String> = row.get(6)?;
                    let identifier: Option<String> = row.get(7)?;

                    let flag = Flag::import(
                        round_id,
                        slot,
                        team_id,
                        service_id,
                        secret,
                        Token::new(token),
                        FlagIdentifier::new(identifier),
                    );
                    Ok((flag_id, flag))
                })?
                .collect::<Result<FlagRegistry, _>>()?;

            Ok(flags)
        }
    }

    pub struct TeamList;

    #[derive(
        Clone, Copy, Serialize, Deserialize, Debug, PartialEq, Eq, Hash, Deref,
    )]
    pub struct TeamId(pub(super) i64);

    impl TeamList {
        pub fn insert(db: &Db, name: String, ip: String) -> anyhow::Result<()> {
            let conn = db.inner()?;
            conn.execute(
                "INSERT INTO teams (name, ip) VALUES (?1, ?2)",
                params![name, ip],
            )
            .context("Failed to insert team into database")?;

            Ok(())
        }

        pub fn get(db: &Db, id: TeamId) -> anyhow::Result<Team> {
            let conn = db.inner()?;
            let mut stmt = conn
                .prepare("SELECT name, ip FROM teams WHERE id = ?")
                .context("Failed to prepare statement for getting team")?;

            let team = stmt
                .query_row([id.0], |row| {
                    let name: String = row.get(0)?;
                    let ip: String = row.get(1)?;

                    Ok(Team::new(ip.parse().unwrap(), name))
                })
                .context("Failed to query team from database")?;

            Ok(team)
        }

        pub fn all(db: &Db) -> anyhow::Result<TeamRegistry> {
            let conn = db.inner()?;
            let mut stmt = conn
                .prepare("SELECT id, name, ip FROM teams")
                .context("Failed to prepare statement for getting all teams")?;

            let teams = stmt
                .query_map([], |row| {
                    let id = TeamId(row.get(0)?);
                    let name: String = row.get(1)?;
                    let ip: String = row.get(2)?;

                    let team = Team::new(ip.parse().unwrap(), name);

                    Ok((id, team))
                })?
                .collect::<Result<Vec<_>, _>>()
                .context("Failed to collect teams from database")?;

            let map: HashMap<TeamId, Team> = teams.into_iter().collect();
            let registry = TeamRegistry::new(map);
            Ok(registry)
        }
    }

    pub struct ServiceList;
    #[derive(
        Clone, Copy, Serialize, Deserialize, Debug, PartialEq, Eq, Hash, Deref,
    )]
    pub struct ServiceId(pub(super) i64);

    impl ServiceList {
        pub fn insert(
            db: &Db,
            name: String,
            port: usize,
            slot: usize,
            timeout: usize,
            concurrency_limit: usize,
            checker_host: String,
            checker_port: usize,
        ) -> anyhow::Result<()> {
            let conn = db.inner()?;
            conn.execute(
                "INSERT INTO services (
                    name, port, slot, timeout, concurrency_limit, checker_host, checker_port
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    name,
                    port,
                    slot,
                    timeout,
                    concurrency_limit,
                    checker_host,
                    checker_port,
                ],
            ).context("Failed to insert service into database")?;

            let service_id = conn.last_insert_rowid();

            // Insert slots for the service
            for slot_number in 1..=slot {
                conn.execute(
                    "INSERT INTO service_slots (service_id, slot_number) VALUES (?1, ?2)",
                    params![service_id, slot_number],
                ).context("Failed to insert service slot into database")?;
            }

            Ok(())
        }

        pub fn get(db: &Db, id: ServiceId) -> anyhow::Result<Service> {
            let conn = db.inner()?;
            let mut stmt = conn.prepare(
                "SELECT name, port, slot, timeout, concurrency_limit, checker_host, checker_port
                 FROM services WHERE id = ?"
            ).context("Failed to prepare statement for getting service")?;

            let service = stmt
                .query_row([id.0], |row| {
                    let name: String = row.get(0)?;
                    let port: u16 = row.get(1)?;
                    let slot: u8 = row.get(2)?;
                    let timeout: usize = row.get(3)?;
                    let concurrency_limit: usize = row.get(4)?;
                    let checker_host: String = row.get(5)?;
                    let checker_port: u16 = row.get(6)?;

                    Ok(Service::new(
                        name,
                        port,
                        slot,
                        timeout,
                        concurrency_limit,
                        checker_host.parse().unwrap(),
                        checker_port,
                    ))
                })
                .context("Failed to query service from database")?;

            Ok(service)
        }

        pub fn all(db: &Db) -> anyhow::Result<ServiceRegistry> {
            let conn = db.inner()?;
            let mut stmt = conn.prepare(
                "SELECT id, name, port, slot, timeout, concurrency_limit, checker_host, checker_port
         FROM services").context("Failed to prepare statement for getting all services")?;

            let services = stmt
                .query_map([], |row| {
                    let id = ServiceId(row.get(0)?);
                    let name = row.get(1)?;
                    let port = row.get(2)?;
                    let slot = row.get(3)?;
                    let timeout = row.get(4)?;
                    let concurrency_limit = row.get(5)?;
                    let checker_host =
                        row.get::<_, String>(6)?.parse().unwrap();
                    let checker_port = row.get(7)?;

                    let service = Service::new(
                        name,
                        port,
                        slot,
                        timeout,
                        concurrency_limit,
                        checker_host,
                        checker_port,
                    );

                    Ok((id, service))
                })?
                .collect::<Result<Vec<_>, _>>()
                .context("Failed to collect services from database")?;

            let map: HashMap<ServiceId, Service> =
                services.into_iter().collect();
            let registry = ServiceRegistry::new(map);
            Ok(registry)
        }
    }

    #[derive(
        Clone, Copy, Serialize, Deserialize, Debug, PartialEq, Eq, Hash,
    )]

    pub struct SlotId {
        id: i64,
        slot_number: i64,
    }

    impl SlotId {
        pub fn get_id(&self) -> i64 {
            self.id
        }

        pub fn get_slot_number(&self) -> i64 {
            self.slot_number
        }
    }

    pub struct ServiceSlots;

    impl ServiceSlots {
        pub fn get(
            db: &Db,
            service_id: ServiceId,
        ) -> anyhow::Result<Vec<SlotId>> {
            let conn = db.inner()?;
            let mut stmt = conn.prepare(
                "SELECT id, slot_number FROM service_slots WHERE service_id = ? ORDER BY slot_number"
            ).context("Failed to prepare statement for getting service slots")?;

            let slots = stmt
                .query_map([service_id.0], |row| {
                    let slot_id: i64 = row.get(0)?;
                    let slot_number: i64 = row.get(1)?;
                    Ok(SlotId {
                        id: slot_id,
                        slot_number,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()
                .context("Failed to collect service slots from database")?;

            Ok(slots)
        }

        pub fn all(db: &Db) -> anyhow::Result<SlotRegistry> {
            let conn = db.inner()?;
            let mut stmt = conn.prepare(
                "SELECT service_id, id, slot_number FROM service_slots ORDER BY service_id, slot_number"
            ).context("Failed to prepare statement for getting all service slots")?;

            let slots = stmt
                .query_map([], |row| {
                    let service_id = ServiceId(row.get(0)?);
                    let slot_id = SlotId {
                        id: row.get(1)?,
                        slot_number: row.get(2)?,
                    };
                    Ok((slot_id, service_id))
                })?
                .collect::<Result<Vec<_>, _>>()
                .context("Failed to collect all service slots from database")?;

            let map: HashMap<SlotId, ServiceId> = slots.into_iter().collect();
            let registry = SlotRegistry::new(map);
            Ok(registry)
        }
    }

    pub struct Config;

    impl Config {
        pub fn insert(
            db: &Db,
            name: &str,
            flag_life: u64,
            tick_time: u64,
            flag_port: u16,
            total_round: u64,
        ) -> anyhow::Result<()> {
            let conn = db.inner()?;
            conn.execute(
            "INSERT OR REPLACE INTO config (id, name, flag_life, tick_time, flag_port, total_round) 
             VALUES (1, ?1, ?2, ?3, ?4, ?5)",
            params![name, flag_life, tick_time, flag_port, total_round],
        ).context("Failed to insert or replace config in database")?;

            Ok(())
        }

        pub fn get(db: &Db) -> anyhow::Result<crate::config::Config> {
            let conn = db.inner()?;
            let mut stmt = conn.prepare(
            "SELECT name, flag_life, tick_time, flag_port, total_round FROM config WHERE id = 1"
        ).context("Failed to prepare statement for getting config")?;

            let config = stmt
                .query_row([], |row| {
                    let name: String = row.get(0)?;
                    let flag_life: u64 = row.get(1)?;
                    let tick_time: u64 = row.get(2)?;
                    let flag_port: u16 = row.get(3)?;
                    let total_round: u64 = row.get(4)?;

                    Ok(crate::config::Config::new(
                        name,
                        flag_life,
                        tick_time,
                        flag_port,
                        total_round,
                    ))
                })
                .context("Failed to query config from database")?;

            Ok(config)
        }
    }
}

pub mod journal {

    use std::str::FromStr;

    use anyhow::Context;
    use chrono::{DateTime, Utc};
    use rusqlite::{params, OptionalExtension};
    use serde::{Deserialize, Serialize};

    use crate::db::tables::{FlagId, ServiceId, TeamId};
    use crate::db::Db;
    use crate::service::{should_update, ServiceState};
    use crate::teams::{Attacker, Victim};

    pub fn initialize_ledger(db: &Db) -> anyhow::Result<()> {
        let conn = db.inner()?;

        // Create a table for storing rounds
        conn.execute(
            "CREATE TABLE IF NOT EXISTS rounds (
                        id INTEGER PRIMARY KEY AUTOINCREMENT,
                        start_time INTEGER NOT NULL,
                        round_number INTEGER NOT NULL,
                        finished BOOLEAN NOT NULL DEFAULT 0
                    )",
            [],
        )
        .context("Failed to create rounds table")?;

        // Create a table for storing service states
        conn.execute(
            "CREATE TABLE IF NOT EXISTS service_states (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                round_id INTEGER NOT NULL,
                team_id INTEGER NOT NULL,
                service_id INTEGER NOT NULL,
                state INTEGER NOT NULL,
                state_message TEXT,
                timestamp INTEGER NOT NULL,
                journal_type TEXT NOT NULL, 
                FOREIGN KEY (round_id) REFERENCES rounds(id),
                FOREIGN KEY (team_id) REFERENCES teams(id),
                FOREIGN KEY (service_id) REFERENCES services(id)
            )",
            [],
        )
        .context("Failed to create service_states table")?;

        // Create a table for tracking flag submissions
        conn.execute(
            "CREATE TABLE IF NOT EXISTS flag_submissions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                round_id INTEGER NOT NULL,
                attacker INTEGER NOT NULL,
                victim INTEGER NOT NULL,
                flag_id INTEGER NOT NULL,
                submission_time INTEGER NOT NULL,
                FOREIGN KEY (round_id) REFERENCES rounds(id),
                FOREIGN KEY (attacker) REFERENCES teams(id),
                FOREIGN KEY (victim) REFERENCES teams(id),
                FOREIGN KEY (flag_id) REFERENCES flags(id)
            )",
            [],
        )
        .context("Failed to create flag_submissions table")?;

        Ok(())
    }

    pub struct RoundJournal;
    #[derive(
        Clone,
        Copy,
        Serialize,
        Deserialize,
        Debug,
        PartialEq,
        Eq,
        Hash,
        PartialOrd,
    )]
    pub struct RoundId {
        pub id: i64,
        pub round_number: i64,
    }

    impl RoundId {
        pub fn is_round_zero(&self) -> bool {
            self.round_number == 0
        }
    }
    impl RoundJournal {
        pub fn start_new_round(db: &Db) -> anyhow::Result<RoundId> {
            let round_number: i64 = match Self::get_last_finished_round(db)? {
                Some(last_round) => last_round.round_number + 1,
                None => 1,
            };

            let conn = db.inner()?;
            let start_time =
                chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();

            conn.execute(
                "INSERT INTO rounds (start_time, round_number, finished) VALUES (?, ?, 0)",
                params![start_time, round_number],
            )?;

            let round_id = conn.last_insert_rowid();
            Ok(RoundId {
                id: round_id,
                round_number,
            })
        }

        pub fn get_current(db: &Db) -> anyhow::Result<RoundId> {
            let conn = db.inner()?;
            let mut stmt = conn.prepare(
                "SELECT id, round_number FROM rounds ORDER BY id DESC LIMIT 1",
            )?;

            let round_id = stmt.query_row([], |row| {
                let id: i64 = row.get(0)?;
                let round_number: i64 = row.get(1)?;
                Ok(RoundId { id, round_number })
            })?;

            Ok(round_id)
        }

        pub fn get_previous(
            db: &Db,
            round_id: RoundId,
        ) -> anyhow::Result<RoundId> {
            let conn = db.inner()?;
            let mut stmt = conn.prepare("SELECT id, round_number FROM rounds WHERE id < ? ORDER BY id DESC LIMIT 1")?;

            let previous_round_id = stmt.query_row([round_id.id], |row| {
                let id: i64 = row.get(0)?;
                let round_number: i64 = row.get(1)?;
                Ok(RoundId { id, round_number })
            })?;

            Ok(previous_round_id)
        }

        pub fn get_by_id(db: &Db, id: i64) -> anyhow::Result<Option<RoundId>> {
            let conn = db.inner()?;
            let mut stmt = conn
                .prepare("SELECT id, round_number FROM rounds WHERE id = ?")?;

            let round_id = stmt
                .query_row([id], |row| {
                    let id: i64 = row.get(0)?;
                    let round_number: i64 = row.get(1)?;
                    Ok(RoundId { id, round_number })
                })
                .optional()?;

            Ok(round_id)
        }

        pub fn mark_round_as_finished(
            db: &Db,
            round_id: RoundId,
        ) -> anyhow::Result<()> {
            let conn = db.inner()?;
            conn.execute(
                "UPDATE rounds SET finished = 1 WHERE id = ?",
                params![round_id.id],
            )?;

            Ok(())
        }

        pub fn get_previous_n_finished(
            db: &Db,
            n: usize,
        ) -> anyhow::Result<Vec<RoundId>> {
            let conn = db.inner()?;
            let mut stmt = conn.prepare(
                "SELECT id, round_number FROM rounds WHERE finished = 1 ORDER BY id DESC LIMIT ?"
            )?;

            let rounds = stmt
                .query_map([n as i64], |row| {
                    let id: i64 = row.get(0)?;
                    let round_number: i64 = row.get(1)?;
                    Ok(RoundId { id, round_number })
                })?
                .collect::<Result<Vec<_>, _>>()?;

            Ok(rounds)
        }

        pub fn get_last_finished_round(
            db: &Db,
        ) -> anyhow::Result<Option<RoundId>> {
            let conn = db.inner()?;
            let mut stmt = conn.prepare("SELECT id, round_number FROM rounds WHERE finished = 1 ORDER BY id DESC LIMIT 1")?;

            let last_round = stmt
                .query_row([], |row| {
                    let id: i64 = row.get(0)?;
                    let round_number: i64 = row.get(1)?;
                    Ok(RoundId { id, round_number })
                })
                .optional()?;

            Ok(last_round)
        }

        pub fn entries(
            db: &Db,
        ) -> anyhow::Result<Vec<(RoundId, chrono::DateTime<chrono::Utc>)>>
        {
            let conn = db.inner()?;
            let mut stmt = conn.prepare(
                "SELECT id, round_number, start_time FROM rounds WHERE finished = 1 ORDER BY id"
            )?;

            let rounds = stmt
                .query_map([], |row| {
                    let id: i64 = row.get(0)?;
                    let round_number: i64 = row.get(1)?;
                    let start_time: i64 = row.get(2)?;
                    let start_time =
                        DateTime::<Utc>::from_timestamp_nanos(start_time);

                    Ok((RoundId { id, round_number }, start_time))
                })?
                .collect::<Result<Vec<_>, _>>()?;

            Ok(rounds)
        }

        pub fn get_start_time(
            db: &Db,
            round_id: RoundId,
        ) -> anyhow::Result<chrono::DateTime<chrono::Utc>> {
            let conn = db.inner()?;
            let mut stmt =
                conn.prepare("SELECT start_time FROM rounds WHERE id = ?")?;

            let start_time: i64 =
                stmt.query_row([round_id.id], |row| row.get(0))?;
            let start_time = DateTime::<Utc>::from_timestamp_nanos(start_time);

            Ok(start_time)
        }
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub struct ServiceStateJournal;

    #[derive(
        strum_macros::EnumString,
        strum_macros::Display,
        Debug,
        Serialize,
        Deserialize,
    )]
    pub enum ServiceStateJournalType {
        // TODO: Remove Error
        Error,
        Initial,
        PlantFlag,
        CheckFlag,
        ServiceCheck,
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub struct ServiceStateJournalEntry {
        pub round_id: RoundId,
        pub team_id: TeamId,
        pub service_id: ServiceId,
        pub state: ServiceState,
        pub state_message: Option<String>,
        pub timestamp: DateTime<Utc>,
        pub journal_type: ServiceStateJournalType,
    }

    impl ServiceStateJournal {
        pub fn append(
            db: &Db,
            round_id: RoundId,
            team_id: TeamId,
            service_id: ServiceId,
            state: ServiceState,
            journal_type: ServiceStateJournalType,
        ) -> anyhow::Result<()> {
            let conn = db.inner()?;

            // Extract ServiceStates
            let state_message = match &state {
                ServiceState::Mumble(msg) | ServiceState::Corrupt(msg) => {
                    Some(msg.clone())
                }
                _ => None,
            };

            let timestamp = chrono::Utc::now();
            conn.execute(
                "INSERT INTO service_states (round_id, team_id, service_id, state, state_message, timestamp, journal_type) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![round_id.id, team_id.0, service_id.0, state.to_num(), state_message, timestamp.timestamp_nanos_opt().unwrap_or_default(), journal_type.to_string()],
            )
            .context("Failed to append service state to ledger")?;
            Ok(())
        }

        pub fn entries(
            db: &Db,
        ) -> anyhow::Result<Vec<ServiceStateJournalEntry>> {
            let conn = db.inner()?;
            // Get the size of the table
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM service_states",
                [],
                |row| row.get(0),
            )?;

            // Prepare the statement
            let mut stmt = conn.prepare(
                "SELECT ss.round_id, ss.team_id, ss.service_id, ss.state, ss.state_message, ss.timestamp, ss.journal_type, r.round_number 
                 FROM service_states ss 
                 JOIN rounds r ON ss.round_id = r.id",
            )?;

            // Pre-allocate the vector with the required size
            let mut entries = Vec::with_capacity(count as usize);

            // Query the rows and map them to ServiceStateJournalEntry
            let rows = stmt.query_map([], |row| {
                let state_num: u8 = row.get(3)?;
                let state_message: Option<String> = row.get(4)?;
                let state = ServiceState::from((state_num, &state_message));
                let journal_type_str: String = row.get(6)?;
                let journal_type =
                    ServiceStateJournalType::from_str(&journal_type_str)
                        .unwrap_or(ServiceStateJournalType::Error);

                Ok(ServiceStateJournalEntry {
                    round_id: RoundId {
                        id: row.get(0)?,
                        round_number: row.get(7)?,
                    },
                    team_id: TeamId(row.get(1)?),
                    service_id: ServiceId(row.get(2)?),
                    state,
                    state_message,
                    timestamp: DateTime::<Utc>::from_timestamp_nanos(
                        row.get(5)?,
                    ),
                    journal_type,
                })
            })?;

            // Collect the successful entries into the pre-allocated vector
            for entry in rows {
                if let Ok(entry) = entry {
                    entries.push(entry);
                }
            }

            Ok(entries)
        }

        pub fn get(
            db: &Db,
            round_id: RoundId,
            team_id: TeamId,
            service_id: ServiceId,
        ) -> anyhow::Result<ServiceState> {
            let conn = db.inner()?;
            let mut stmt = conn.prepare(
                "SELECT state, state_message FROM service_states WHERE 
                      round_id = ?1 AND team_id = ?2 AND service_id = ?3 ORDER BY timestamp ASC",
            )?;

            let rows = stmt.query_map(
                params![round_id.id, team_id.0, service_id.0],
                |row| {
                    let state_num: u8 = row.get(0)?;
                    let state_message: Option<String> = row.get(1)?;
                    let state = ServiceState::from((state_num, &state_message));
                    Ok(state)
                },
            )?;

            let states: Vec<ServiceState> =
                rows.filter_map(Result::ok).collect();

            let mut final_state = ServiceState::Unknown;
            for state in states {
                if should_update(&final_state, &state) {
                    final_state = state;
                }
            }

            Ok(final_state)
        }
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub struct FlagSubmissionJournal;

    #[derive(Debug, Serialize, Deserialize)]
    pub struct FlagSubmissionJournalEntry {
        pub round_id: RoundId,
        pub attacker: Attacker,
        pub victim: Victim,
        pub flag_id: FlagId,
        pub submission_time: DateTime<Utc>,
    }
    impl FlagSubmissionJournal {
        pub fn append(
            db: &Db,
            entry: FlagSubmissionJournalEntry,
        ) -> anyhow::Result<()> {
            let conn = db.inner()?;
            conn.execute(
                "INSERT INTO flag_submissions (round_id, attacker, victim, flag_id, submission_time) VALUES 
                    (?1, ?2, ?3, ?4, ?5)",
                params![entry.round_id.id, entry.attacker.id().0, entry.victim.0, entry.flag_id.0, entry.submission_time.timestamp_nanos_opt().unwrap_or_default()],
            )
            .context("Failed to append flag submission to ledger")?;
            Ok(())
        }

        pub fn entries(
            db: &Db,
        ) -> anyhow::Result<Vec<FlagSubmissionJournalEntry>> {
            let conn = db.inner()?;

            // Get the size of the table
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM flag_submissions",
                [],
                |row| row.get(0),
            )?;

            // Prepare the statement
            let mut stmt = conn.prepare(
                "SELECT fs.round_id, fs.attacker, fs.victim, fs.flag_id, fs.submission_time, r.round_number
                 FROM flag_submissions fs
                 JOIN rounds r ON fs.round_id = r.id
                 WHERE r.finished = 1",
            )?;

            // Pre-allocate the vector with the required size
            let mut entries = Vec::with_capacity(count as usize);

            // Query the rows and map them to FlagSubmissionEntry
            let rows = stmt.query_map([], |row| {
                let submission_time = row.get(4)?;
                Ok(FlagSubmissionJournalEntry {
                    round_id: RoundId {
                        id: row.get(0)?,
                        round_number: row.get(5)?,
                    },
                    attacker: Attacker::new(TeamId(row.get(1)?)),
                    victim: Victim::new(TeamId(row.get(2)?)),
                    flag_id: FlagId(row.get(3)?),
                    submission_time: DateTime::<Utc>::from_timestamp_nanos(
                        submission_time,
                    ),
                })
            })?;

            // Collect the successful entries into the pre-allocated vector
            for entry in rows {
                if let Ok(entry) = entry {
                    entries.push(entry);
                }
            }

            Ok(entries)
        }
    }
}

#[derive(Clone)]
pub struct Db(Arc<Mutex<rusqlite::Connection>>);

impl Db {
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Db(Arc::new(Mutex::new(
            rusqlite::Connection::open(path)
                .context("Failed to open database connection")?,
        ))))
    }

    pub fn inner(&self) -> Result<MutexGuard<rusqlite::Connection>> {
        self.0
            .lock()
            .map_err(|e| anyhow::anyhow!("Failed to lock the database: {}", e))
    }
}
