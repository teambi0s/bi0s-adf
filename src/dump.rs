use std::{collections::HashMap, fs::File, path::PathBuf};

use anyhow::Context;
use serde::Serialize;

use crate::{
    db::{
        journal::{
            FlagSubmissionJournal, RoundId, RoundJournal, ServiceStateJournal,
        },
        tables::{
            Config, FlagStore, ServiceId, ServiceList, ServiceSlots, TeamId,
            TeamList,
        },
        Db,
    },
    flag::{DefaultFlag, FlagCodec},
    service::{self, Service, ServiceState},
    teams::Team,
};

pub struct Dump {
    db: Db,
    location: PathBuf,
}

impl Dump {
    pub fn new(db: Db, location: PathBuf) -> Self {
        Self { db, location }
    }

    pub fn teams(&self) -> anyhow::Result<()> {
        let team_registry = TeamList::all(&self.db)?;
        let teams = team_registry.values().collect::<Vec<&Team>>();
        let mut location = self.location.clone();
        location.push("teams.json");

        let file = File::create(location)
            .context("Failed to create file for dumping teams")?;

        serde_json::to_writer_pretty(file, &teams)
            .context("Failed to write teams to JSON")?;

        Ok(())
    }

    pub fn services(&self) -> anyhow::Result<()> {
        let service_registry = ServiceList::all(&self.db)?;
        let services = service_registry.values().collect::<Vec<&Service>>();
        let mut location = self.location.clone();
        location.push("services.json");

        let file = File::create(location)
            .context("Failed to create file for dumping services")?;

        serde_json::to_writer_pretty(file, &services)
            .context("Failed to write services to JSON")?;

        Ok(())
    }

    pub fn config(&self) -> anyhow::Result<()> {
        let last_round =
            RoundJournal::get_last_finished_round(&self.db)?.unwrap();

        #[derive(Serialize)]
        struct Res {
            name: String,
            flag_life: u64,
            tick_time: u64,
            flag_port: u16,
            total_round: u64,
            last_finished_round: i64,
        }

        let config = Config::get(&self.db)?;
        let mut location = self.location.clone();
        location.push("config.json");

        let config = Res {
            name: config.name().to_string(),
            flag_life: config.flag_life(),
            tick_time: config.tick_time(),
            flag_port: config.flag_port(),
            total_round: config.total_round(),
            last_finished_round: last_round.round_number,
        };

        let file = File::create(location)
            .context("Failed to create file for dumping config")?;

        serde_json::to_writer_pretty(file, &config)
            .context("Failed to write config to JSON")?;

        Ok(())
    }

    pub fn flags(&self) -> anyhow::Result<()> {
        #[derive(Serialize)]
        struct Res {
            team: String,
            service: String,
            slot: u64,
            flag: Vec<(String, String)>,
        }

        let team_registry = TeamList::all(&self.db)?;
        let service_registry = ServiceList::all(&self.db)?;

        let mut location = self.location.clone();
        location.push("flags.json");

        let flags = FlagStore::all(&self.db)?;

        let mut result: Vec<Res> = Vec::new();
        for team_id in team_registry.ids() {
            for service_id in service_registry.ids() {
                let slot_ids = ServiceSlots::get(&self.db, service_id)?;
                for slot_id in slot_ids {
                    let mut team_flags = Vec::new();

                    let team =
                        team_registry.get(&team_id).expect("Invalid TeamId");
                    let service = service_registry
                        .get(&service_id)
                        .expect("Invalid ServiceId");

                    for flag in flags.values() {
                        if flag.team_id() == team_id
                            && flag.service_id() == service_id
                            && flag.slot() == slot_id
                        {
                            team_flags.push((
                                DefaultFlag::encode(flag.secret()),
                                flag.identifier().to_string(),
                            ))
                        }
                    }

                    let res = Res {
                        team: String::from(team.name()),
                        service: String::from(service.name()),
                        slot: slot_id.get_slot_number() as u64,
                        flag: team_flags,
                    };

                    result.push(res);
                }
            }
        }

        let file = File::create(location)
            .context("Failed to create file for dumping flags")?;

        serde_json::to_writer_pretty(file, &result)
            .context("Failed to write flags to JSON")?;

        Ok(())
    }

    pub fn flag_submissions(&self) -> anyhow::Result<()> {
        let submissions = FlagSubmissionJournal::entries(&self.db)?;

        let team_registry = TeamList::all(&self.db)?;
        let flags_registry = FlagStore::all(&self.db)?;

        #[derive(Serialize)]
        struct Submission {
            submission_time: String,
            round: i64,
            attacker: String,
            victim: String,
            flag: String,
        }

        let mapped_submissions: Vec<Submission> = submissions
            .into_iter()
            .map(|entry| {
                let attacker_name = team_registry
                    .get(&entry.attacker)
                    .expect("Invalid TeamId specified")
                    .name()
                    .to_owned();

                let victim_name = team_registry
                    .get(&entry.victim)
                    .expect("Invalid TeamId specified")
                    .name()
                    .to_owned();

                let flag = flags_registry
                    .get(&entry.flag_id)
                    .expect("Invalid FlagId specified");

                Submission {
                    submission_time: entry.submission_time.to_string(),
                    round: entry.round_id.round_number,
                    attacker: attacker_name,
                    victim: victim_name,
                    flag: DefaultFlag::encode(flag.secret()),
                }
            })
            .collect();

        let mut location = self.location.clone();
        location.push("flag_submissions.json");

        let file = File::create(location)
            .context("Failed to create file for dumping flag submissions")?;

        serde_json::to_writer_pretty(file, &mapped_submissions)
            .context("Failed to write flag submissions to JSON")?;

        Ok(())
    }

    pub fn service_states(&self) -> anyhow::Result<()> {
        let team_registry = TeamList::all(&self.db)?;
        let service_registry = ServiceList::all(&self.db)?;
        let rounds: Vec<RoundId> = RoundJournal::entries(&self.db)?
            .into_iter()
            .map(|(round_id, _)| round_id)
            .collect();

        let entries = ServiceStateJournal::entries(&self.db)?;

        let mut state_map: HashMap<(RoundId, TeamId, ServiceId), ServiceState> =
            HashMap::new();

        for entry in entries {
            let key = (entry.round_id, entry.team_id, entry.service_id);
            state_map
                .entry(key)
                .and_modify(|previous_state| {
                    if service::should_update(&*previous_state, &entry.state) {
                        *previous_state = entry.state.clone();
                    }
                })
                .or_insert(entry.state);
        }

        // Prepare the final output
        #[derive(Serialize)]
        struct Entry {
            team: String,
            service: String,
            service_states: Vec<(i64, String)>,
        }

        let mut result = Vec::new();
        for team_id in team_registry.ids() {
            for service_id in service_registry.ids() {
                let team = team_registry
                    .get(&team_id)
                    .expect("Invalid TeamId specified")
                    .name()
                    .to_owned();
                let service = service_registry
                    .get(&service_id)
                    .expect("Invalid ServiceId specified")
                    .name()
                    .to_owned();

                let mut service_states = Vec::with_capacity(rounds.len());
                for round_id in &rounds {
                    if round_id.round_number == 0 {
                        continue;
                    }

                    let service_state = state_map
                        .get(&(*round_id, team_id, service_id))
                        .expect("No ServiceState found for specified RoundId");
                    service_states.push((
                        round_id.round_number,
                        String::from(service_state),
                    ));
                }

                let entry = Entry {
                    team,
                    service,
                    service_states,
                };
                result.push(entry);
            }
        }

        let mut location = self.location.clone();
        location.push("service_states.json");

        let file = File::create(location)
            .context("Failed to create file for dumping service states")?;

        serde_json::to_writer_pretty(file, &result)
            .context("Failed to write service states to JSON")?;

        Ok(())
    }

    pub fn rounds(&self) -> anyhow::Result<()> {
        // Retrieve all round entries
        let rounds = RoundJournal::entries(&self.db)?;

        // Prepare the final output
        #[derive(Serialize)]
        struct RoundEntry {
            round_number: i64,
            start_time: String,
        }

        let mapped_rounds: Vec<RoundEntry> = rounds
            .into_iter()
            .map(|(round_id, start_time)| RoundEntry {
                round_number: round_id.round_number,
                start_time: start_time.to_string(),
            })
            .collect();

        let mut location = self.location.clone();
        location.push("rounds.json");

        let file = File::create(location)
            .context("Failed to create file for dumping rounds")?;

        serde_json::to_writer_pretty(file, &mapped_rounds)
            .context("Failed to write rounds to JSON")?;

        Ok(())
    }

    pub fn dump_everything(&self) -> anyhow::Result<()> {
        self.teams()?;
        self.services()?;
        self.config()?;

        self.rounds()?;
        self.flags()?;

        self.service_states()?;
        self.flag_submissions()?;

        Ok(())
    }
}
