use std::sync::Arc;

use tokio::sync::{
    mpsc::{self, Receiver, Sender},
    RwLock,
};

use serde::{Deserialize, Serialize};

use crate::{
    db::{
        journal::RoundId,
        tables::{ServiceId, TeamId},
    },
    flag::FlagKey,
    history::History,
    service::{ServiceRegistry, ServiceState},
    teams::{Attacker, TeamRegistry},
};

use tracing::info;

/// This trait specifies the updates that should be made to the score of the teams
/// on different events.
pub trait Score {
    fn new(teams: TeamRegistry, service: ServiceRegistry) -> Self;

    /// Defines how the score should be updated on a change in the ServiceState of services.
    fn on_service_state_update(
        &mut self,
        team_id: TeamId,
        service_id: ServiceId,
        round_id: RoundId,
        service_state: ServiceState,
    );

    /// Defines how the score should be updated on flag submission by a specific
    /// team.
    fn on_flag_submission(&mut self, attacker: Attacker, flag: FlagKey);

    /// This function is called when starting a new round.
    fn on_new_round(&mut self, round_id: RoundId);

    /// Public view of the score. This function will be called by the API
    /// module to serve the scoreboard to the players.
    fn view(&self) -> ScoreBoard;
}

pub struct Model {
    scoreboard: RwLock<ScoreBoard>,
    pub h_score: RwLock<History<f64>>,
}

impl Model {
    pub async fn scoreboard(&self) -> ScoreBoard {
        self.scoreboard.read().await.clone()
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum Msg {
    UpdateScoreboard,

    // These messages update the internal state of the bot
    // for having a consistent state while replying. The messages
    // should also come in the same order as before.
    ServiceStateUpdated(TeamId, ServiceId, RoundId, ServiceState),
    FlagSubmission(Attacker, FlagKey),
    UpdateRound(RoundId),
}

pub struct ScoreBot<S> {
    /// Internal representation of the score
    score: S,

    /// Communication channel
    comm: (Sender<Msg>, Receiver<Msg>),
    /// Internal data used in this bot
    model: Arc<Model>,
}

impl<S: Score + Clone> ScoreBot<S> {
    pub fn new(
        teams: TeamRegistry,
        services: ServiceRegistry,
    ) -> anyhow::Result<Self> {
        let comm = mpsc::channel(10);
        let score = S::new(teams, services);
        let model = Arc::new(Model {
            scoreboard: RwLock::new(ScoreBoard::new()),
            h_score: RwLock::new(History::new()),
        });
        Ok(Self { score, comm, model })
    }

    pub fn sender(&self) -> Sender<Msg> {
        self.comm.0.clone()
    }

    pub fn model(&self) -> Arc<Model> {
        self.model.clone()
    }

    pub async fn run(mut self) {
        info!("Starting ScoreBot");

        // Handle incoming messages.
        while let Some(msg) = self.comm.1.recv().await {
            // info!("Got ScoreBot Msg : {}", &msg);
            match msg {
                Msg::ServiceStateUpdated(
                    team_id,
                    service_id,
                    round_id,
                    service_state,
                ) => {
                    self.score.on_service_state_update(
                        team_id,
                        service_id,
                        round_id,
                        service_state,
                    );
                }
                Msg::FlagSubmission(attacker, flag) => {
                    self.score.on_flag_submission(attacker, flag);
                }
                Msg::UpdateRound(round_id) => {
                    {
                        if round_id.round_number == 0 {
                            continue;
                        }

                        let mut h_score = self.model.h_score.write().await;
                        let scoreboard = self.score.view();
                        let round = round_id.clone();

                        for team_score in scoreboard.0 {
                            let team_id = team_score.team_id;
                            for service_score in team_score.service_score {
                                let service_id = service_score.service_id;
                                h_score.insert(
                                    team_id,
                                    service_id,
                                    round,
                                    team_score.total_point,
                                )
                            }
                        }
                    }
                    self.score.on_new_round(round_id);
                }

                Msg::UpdateScoreboard => {
                    let score = self.score.view();
                    let mut board = self.model.scoreboard.write().await;
                    *board = score;
                }
            }
        }
    }
}

impl std::fmt::Display for Msg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Msg::FlagSubmission(..) => write!(f, "FlagSubmission"),
            Msg::UpdateRound(..) => write!(f, "UpdateRound"),
            Msg::ServiceStateUpdated(..) => write!(f, "ServiceStateUpdated"),
            Msg::UpdateScoreboard => write!(f, "UpdateScoreboard"),
        }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct TeamScoreBoard {
    pub team_id: TeamId,
    pub position: usize,
    pub total_point: f64,
    pub attack_point: f64,
    pub defence_point: f64,
    pub sla: f64,
    pub service_score: Vec<ServiceScoreBoard>,
}
#[derive(Debug, Serialize, Clone)]
pub struct ServiceScoreBoard {
    pub service_id: ServiceId,
    pub attack_point: f64,
    pub defence_point: f64,
    pub sla: f64,
}

#[derive(Debug, Serialize, Clone)]
pub struct ScoreBoard(pub Vec<TeamScoreBoard>);

impl ScoreBoard {
    pub fn new() -> Self {
        Self(Vec::new())
    }
}

// TODO: Optimize the scoring calculation. Currently, we are recalculating everything.
pub mod faust {
    use std::collections::{HashMap, HashSet};

    use crate::{
        db::{
            journal::RoundId,
            tables::{ServiceId, TeamId},
        },
        flag::FlagKey,
        service::{should_update, ServiceRegistry, ServiceState},
        teams::{Attacker, TeamRegistry},
    };

    use super::{Score, ScoreBoard, ServiceScoreBoard, TeamScoreBoard};
    use bitvec::prelude::BitVec;

    // Scoring algorithm from
    // https://web.archive.org/web/20240925182250/https://2024.faustctf.net/information/rules/

    #[derive(Clone)]
    pub struct FaustCTF {
        teams: TeamRegistry,
        service: ServiceRegistry,
        flag_weight: HashMap<FlagKey, BitVec>,
        flag_submission: HashMap<TeamId, HashSet<FlagKey>>,
        round_ids: Vec<RoundId>,

        service_state: HashMap<(TeamId, ServiceId, RoundId), u8>,
        current_round: Option<RoundId>,
    }

    impl From<&FaustCTF> for ScoreBoard {
        fn from(value: &FaustCTF) -> Self {
            let mut team_scoreboard = Vec::new();
            for team_id in value.teams.ids() {
                let mut service_scores =
                    Vec::with_capacity(value.service.len());
                for service_id in value.service.ids() {
                    let attack_point =
                        value.calculate_attack_point(team_id, service_id);
                    let defence_point =
                        value.calculate_defence_point(team_id, service_id);
                    let sla = value.calculate_sla(team_id, service_id);

                    let service_score = ServiceScoreBoard {
                        service_id,
                        attack_point,
                        defence_point,
                        sla,
                    };
                    service_scores.push(service_score)
                }
                let attack_point =
                    service_scores.iter().map(|score| score.attack_point).sum();
                let defence_point = service_scores
                    .iter()
                    .map(|score| score.defence_point)
                    .sum();
                let sla = service_scores.iter().map(|score| score.sla).sum();
                let total_point = attack_point + defence_point + sla;
                let teamscore = TeamScoreBoard {
                    team_id,
                    position: 0,
                    total_point,
                    attack_point,
                    defence_point,
                    sla,
                    service_score: service_scores,
                };

                team_scoreboard.push(teamscore);
            }

            // Sort the teams by their total score in descending order and assign positions
            // TODO: How to sort the scoring ????

            team_scoreboard.sort_by(|a, b| {
                b.total_point.partial_cmp(&a.total_point).unwrap()
            });

            for (position, teamscore) in team_scoreboard.iter_mut().enumerate()
            {
                teamscore.position = position + 1;
            }

            ScoreBoard(team_scoreboard)
        }
    }

    impl FaustCTF {
        fn calculate_attack_point(
            &self,
            team_id: TeamId,
            service_id: ServiceId,
        ) -> f64 {
            let mut offence = 0.0;
            if let Some(set) = self.flag_submission.get(&team_id) {
                // offence = set.len() as f64;
                for flag_key in set.iter() {
                    if flag_key.service_id() != service_id {
                        continue;
                    }

                    offence += 1.0;

                    let flag_weight = self
                        .flag_weight
                        .get(flag_key)
                        .map(|bitvec| bitvec.iter().filter(|bit| **bit).count())
                        .unwrap_or_default();

                    offence += 1.0 / flag_weight as f64;
                }
            }
            offence
        }

        fn calculate_defence_point(
            &self,
            team_id: TeamId,
            service_id: ServiceId,
        ) -> f64 {
            let mut defence = 0.0;

            for (flag, bitvec) in self.flag_weight.iter() {
                if flag.team_id() == team_id && flag.service_id() == service_id
                {
                    let count =
                        bitvec.iter().filter(|bit| **bit).count() as f64;
                    defence -= count.powf(0.75)
                }
            }

            defence
        }

        fn calculate_sla(&self, team_id: TeamId, service_id: ServiceId) -> f64 {
            let mut up_count = 0.0;
            for round_id in self.round_ids.iter().cloned() {
                if let Some(current_round) = self.current_round {
                    if round_id.round_number == current_round.round_number {
                        continue;
                    }
                }
                up_count += self
                    .service_state
                    .get(&(team_id, service_id, round_id))
                    .map(|state| {
                        if *state == ServiceState::Up.to_num() {
                            1.0
                        } else if *state == ServiceState::Recovering.to_num() {
                            0.5
                        } else {
                            0.0
                        }
                    })
                    .unwrap_or_default()
            }

            let sla = up_count * (self.teams.len() as f64).sqrt();
            sla
        }
    }

    impl Score for FaustCTF {
        fn new(teams: TeamRegistry, service: ServiceRegistry) -> Self {
            let service_state = HashMap::new();
            let round_ids = Vec::with_capacity(400);
            let flag_weight = HashMap::new();
            let mut flag_submission = HashMap::new();
            for team_id in teams.ids() {
                flag_submission.insert(team_id, HashSet::new());
            }

            Self {
                teams,
                service,
                flag_weight,
                flag_submission,
                round_ids,
                service_state,
                current_round: None,
            }
        }

        fn on_flag_submission(&mut self, attacker: Attacker, flag: FlagKey) {
            self.flag_submission.entry(attacker.id()).and_modify(|set| {
                set.insert(flag.clone());
            });

            // NOTE: Here we assume that the team_ids are sequential and start with id 1.
            // TODO: Add asserts to check if the attacker.id is within bounds
            self.flag_weight
                .entry(flag)
                .and_modify(|bitvec| {
                    bitvec.set((*attacker.id() - 1) as usize, true)
                })
                .or_insert({
                    let mut bitvec = bitvec::bitvec![0; self.teams.len()];
                    bitvec.set((*attacker.id() - 1) as usize, true);
                    bitvec
                });
        }

        fn on_service_state_update(
            &mut self,
            team_id: TeamId,
            service_id: ServiceId,
            round_id: RoundId,
            service_state: ServiceState,
        ) {
            self.service_state
                .entry((team_id, service_id, round_id))
                .and_modify(|previous_state| {
                    let new_state = &service_state;
                    if should_update(*previous_state, new_state) {
                        *previous_state = new_state.to_num();
                    }
                })
                .or_insert(service_state.to_num());
        }

        fn on_new_round(&mut self, round_id: RoundId) {
            self.current_round = Some(round_id);
            self.round_ids.push(round_id);
        }

        fn view(&self) -> super::ScoreBoard {
            ScoreBoard::from(self)
        }
    }
}

pub mod ru_ctf {
    use std::collections::HashMap;

    use super::{Score, ScoreBoard, ServiceScoreBoard, TeamScoreBoard};
    use crate::{
        db::journal::RoundId,
        db::tables::{ServiceId, TeamId},
        flag::FlagKey,
        service::{ServiceRegistry, ServiceState},
        teams::{Attacker, TeamRegistry},
    };
    use serde::Serialize;

    /// Scoring used for RuCTF
    /// https://ructfe.org/rules/
    #[derive(Debug, Clone)]
    pub struct RuCTF {
        score: HashMap<TeamId, TeamScore>,
        position: Vec<(f64, TeamId)>,
    }

    impl From<RuCTF> for ScoreBoard {
        fn from(ru: RuCTF) -> Self {
            let mut ret = Vec::new();
            for (position, team_id) in
                ru.position.into_iter().map(|x| x.1).enumerate()
            {
                let team_score = ru.score.get(&team_id).unwrap();
                let flagpoint = team_score.flagpoint;
                let sla = team_score.sla;
                let mut service = Vec::new();
                for (service_id, service_score) in
                    team_score.service_score.iter()
                {
                    let service_id = *service_id;
                    service.push(ServiceScoreBoard {
                        service_id,
                        attack_point: service_score.flagpoint,
                        defence_point: 0.0,
                        sla: service_score.sla,
                    });
                }

                let total_point = flagpoint * sla;
                ret.push(TeamScoreBoard {
                    team_id,
                    position: position + 1,
                    total_point,
                    attack_point: flagpoint,
                    defence_point: 0.0,
                    sla,
                    service_score: service,
                })
            }
            ScoreBoard(ret)
        }
    }
    #[derive(Debug, Serialize, Clone)]
    struct TeamScore {
        flagpoint: f64,
        sla: f64,
        service_score: HashMap<ServiceId, ServiceScore>,
    }

    #[derive(Debug, Serialize, Clone)]
    struct ServiceScore {
        flagpoint: f64,
        sla: f64,
        status: HashMap<RoundId, bool>,
    }

    impl TeamScore {
        fn calculate_score(&mut self) {
            let mut flagpoint = 0.0;
            let mut sla = 0.0;
            for score in self.service_score.values() {
                flagpoint += score.flagpoint;
                sla += score.sla;
            }
            self.flagpoint = flagpoint;
            self.sla = sla;
        }
    }

    impl ServiceScore {
        fn update_status(&mut self, round: RoundId, status: bool) {
            self.status
                .entry(round)
                .and_modify(|entry| *entry = status)
                .or_insert(status);
            self.calculate_sla();
        }

        fn calculate_sla(&mut self) {
            let mut sla = 0.0;
            let total = self.status.len();
            for status in self.status.values() {
                sla += if *status { 1.0 } else { 0.0 }
            }
            self.sla = sla / (total as f64);
        }
    }

    impl RuCTF {
        pub fn position(&self, team_id: TeamId) -> Option<usize> {
            self.position.iter().position(|x| x.1 == team_id)
        }

        pub fn update_position(&mut self) {
            for team in self.position.iter_mut() {
                if let Some(s) = self.score.get(&team.1) {
                    team.0 = s.flagpoint * s.sla;
                }
            }
            self.position.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        }
    }

    impl Score for RuCTF {
        fn new(teams: TeamRegistry, services: ServiceRegistry) -> Self {
            let position =
                teams.ids().into_iter().map(|x| (0.0, x.clone())).collect();
            let mut score = HashMap::new();
            for team in teams.ids() {
                let mut service_score = HashMap::new();
                for service in services.ids() {
                    let s = ServiceScore {
                        flagpoint: 0.0,
                        sla: 1.0,
                        status: HashMap::new(),
                    };
                    service_score.insert(service, s);
                }
                let team_score = TeamScore {
                    flagpoint: 0.0,
                    sla: 1.0,
                    service_score,
                };
                score.insert(team.clone(), team_score);
            }

            RuCTF { score, position }
        }

        fn on_flag_submission(&mut self, attacker: Attacker, flag: FlagKey) {
            let victim = flag.team_id();
            let attacker_pos = self.position(*attacker).unwrap();
            let victim_pos = self.position(victim).unwrap();

            let service_id = flag.service_id();

            let max = self.score.len() as f64;

            let flag_score = if attacker_pos > victim_pos {
                max
            } else {
                (max.log10()
                    * ((max - victim_pos as f64) / (max - attacker_pos as f64)))
                    .exp()
            };

            if let Some(team_score) = self.score.get_mut(&*attacker) {
                if let Some(s) = team_score.service_score.get_mut(&service_id) {
                    s.flagpoint += flag_score
                }
                team_score.calculate_score();
            }

            if let Some(team_score) = self.score.get_mut(&victim) {
                if let Some(s) = team_score.service_score.get_mut(&service_id) {
                    s.flagpoint -= flag_score.min(s.flagpoint)
                }
                team_score.calculate_score();
            }
            self.update_position();
        }

        fn on_service_state_update(
            &mut self,
            team_id: TeamId,
            service_id: ServiceId,
            round_id: RoundId,
            service_state: ServiceState,
        ) {
            if let Some(team_score) = self.score.get_mut(&team_id) {
                if let Some(service_score) =
                    team_score.service_score.get_mut(&service_id)
                {
                    let status = match service_state {
                        ServiceState::Up => true,
                        _ => false,
                    };
                    service_score.update_status(round_id, status);
                }
                team_score.calculate_score();
            }
            self.update_position();
        }

        fn on_new_round(&mut self, _round_id: RoundId) {}

        fn view(&self) -> ScoreBoard {
            let scoreboard = ScoreBoard::from(self.clone());
            scoreboard
        }
    }
}
