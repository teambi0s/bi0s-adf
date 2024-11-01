use cached::proc_macro::once;
use serde::Serialize;
use std::{collections::HashMap, net::IpAddr, sync::Arc, time::Duration};
use tokio::{sync::RwLock, time};

use crate::{
    db::{
        journal::{self, RoundId, RoundJournal},
        tables::{FlagStore, ServiceId, TeamId},
    },
    game::{Comm, Model},
    scoring,
    service::{ServiceRegistry, ServiceState, ServiceStateKey},
    teams::TeamRegistry,
};

pub struct Cache {
    pub teams: String,
    pub services: String,

    pub scoreboard: RwLock<String>,
}

impl Cache {
    pub fn new(teams: &TeamRegistry, services: &ServiceRegistry) -> Self {
        let mut t = Vec::new();

        for (team_id, team) in teams.inner().iter() {
            #[derive(Serialize)]
            struct Res {
                team_id: String,
                name: String,
                ip: IpAddr,
            }
            let res = Res {
                team_id: (*team_id).to_string(),
                name: team.name().to_string(),
                ip: team.ip(),
            };
            t.push(res)
        }

        let mut s = Vec::new();
        for (service_id, service) in services.inner().iter() {
            #[derive(Serialize)]
            struct Res {
                service_id: String,
                name: String,
                port: u16,
            }
            let res = Res {
                service_id: (*service_id).to_string(),
                name: String::from(service.name()),
                port: service.port(),
            };
            s.push(res)
        }

        let t = serde_json::to_string_pretty(&t)
            .unwrap_or("Unable to serialize TeamsList".to_string());
        let s = serde_json::to_string_pretty(&s)
            .unwrap_or("Unable to serialize ServiceList".to_string());

        let mut h_service_state: HashMap<(TeamId, ServiceId), Vec<u8>> =
            HashMap::new();
        for team_id in teams.ids() {
            for service_id in services.ids() {
                h_service_state.insert((team_id, service_id), Vec::new());
            }
        }

        Self {
            teams: t,
            services: s,
            scoreboard: RwLock::new(String::new()),
        }
    }
}

pub async fn update_cache(
    cache: Arc<Cache>,
    comm: Comm,
    model: Arc<Model>,
) -> anyhow::Result<()> {
    let mut interval = time::interval(Duration::from_millis(500));
    loop {
        interval.tick().await;

        // Update Scoreboard
        comm.scorebot(scoring::Msg::UpdateScoreboard).await;
        let new_scoreboard = model.scorebot_model().scoreboard().await;
        *cache.scoreboard.write().await =
            serde_json::to_string_pretty(&new_scoreboard).unwrap_or_default();
    }
}

// #[once(time = 1, option = true, sync_writes = true)]
pub fn get_service_state(model: Arc<Model>) -> Option<String> {
    #[derive(Serialize)]
    struct Inner {
        service_id: ServiceId,
        state: String,
        reason: Option<String>,
    }

    #[derive(Serialize)]
    struct Res {
        team_id: TeamId,
        service_states: Vec<Inner>,
    }

    let mut result = Vec::new();
    let current_round = journal::RoundJournal::get_current(&model.db).ok()?;
    let previous_round =
        journal::RoundJournal::get_previous(&model.db, current_round).ok()?;

    for team_id in model.teams.ids() {
        let mut service_states = Vec::new();
        for service_id in model.services.ids() {
            let mut service_state = ServiceState::Unknown;
            let res = journal::ServiceStateJournal::get(
                &model.db,
                previous_round,
                team_id,
                service_id,
            );

            if let Ok(state) = res {
                service_state = state;
            }

            let inner = Inner {
                service_id,
                state: String::from(service_state.to_state().to_uppercase()),
                reason: service_state.reason().cloned(),
            };

            service_states.push(inner);
        }

        result.push(Res {
            team_id,
            service_states,
        })
    }

    Some(serde_json::to_string_pretty(&result).unwrap_or_default())
}

#[once(time = 1, option = true, sync_writes = true)]
pub async fn get_service_state_history(
    team_id: TeamId,
    service_id: ServiceId,
    model: Arc<Model>,
) -> Option<String> {
    let rounds: Vec<RoundId> = journal::RoundJournal::entries(&model.db)
        .ok()?
        .into_iter()
        .map(|(round_id, _start_time)| round_id)
        .collect();

    let map = model.service_state_map.read().await;
    let service_states: Vec<u8> = rounds
        .iter()
        .map(|round_id| {
            let key = ServiceStateKey {
                team_id,
                service_id,
                round_id: *round_id,
            };
            map.get(&key).map(|service_state| service_state.to_num())
        })
        // TODO: Is this okay? We are ignoring unknown service_state for a round_id,
        // which ideally should not happen.
        .flatten()
        .to_owned()
        .collect();

    Some(serde_json::to_string(&service_states).unwrap_or_default())
}

// #[once(time = 1, option = true, sync_writes = true)]
pub fn get_flag_identifier(
    team_id: TeamId,
    service_id: ServiceId,
    model: Arc<Model>,
) -> Option<String> {
    let rounds = RoundJournal::get_previous_n_finished(
        &model.db,
        (model.config.flag_life() + 10) as usize,
    )
    .ok()?;

    #[derive(Serialize)]
    struct Inner {
        round: i64,
        identifier: Vec<String>,
    }

    #[derive(Serialize)]
    struct Res {
        team_id: TeamId,
        service_id: ServiceId,
        flag_ids: Vec<Inner>,
    }

    let mut flag_map = HashMap::new();
    for round in rounds {
        if round.is_round_zero() {
            continue;
        }
        let flags = FlagStore::get_by_round(&model.db, round).ok()?;
        flag_map.insert(round, flags);
    }

    let mut flag_ids = Vec::new();
    for (round, flags) in &flag_map {
        let mut identifier = Vec::new();
        for (_id, flag) in flags {
            assert_eq!(round.round_number, flag.round_id().round_number);
            if flag.team_id() == team_id {
                if flag.service_id() == service_id {
                    if flag.identifier().is_none() {
                        continue;
                    }
                    identifier.push(flag.identifier().to_string());
                }
            }
        }
        let inner = Inner {
            round: round.round_number,
            identifier,
        };

        flag_ids.push(inner);
    }

    flag_ids.sort_by(|a, b| a.round.cmp(&b.round));

    let res = Res {
        team_id,
        service_id,
        flag_ids,
    };

    Some(serde_json::to_string_pretty(&res).unwrap_or_default())
}
