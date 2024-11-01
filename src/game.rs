use crate::{
    api::game_api,
    checker::{self, ResponseMsg},
    config::Config,
    db::{
        journal::{
            self, FlagSubmissionJournalEntry, RoundId,
            ServiceStateJournalEntry, ServiceStateJournalType,
        },
        tables::{self, FlagId, FlagStore, ServiceId, TeamId},
        Db,
    },
    flag::{Flag, FlagCodec, FlagKey, FlagRegistry, FlagSubmissionSet},
    flagbot::{self, FlagBot, FlagEvent},
    scoring::{self, Score, ScoreBot},
    service::{
        ServiceRegistry, ServiceState, ServiceStateKey, ServiceStateMap,
        SlotRegistry,
    },
    teams::{TeamRegistry, Victim},
};

use anyhow::{anyhow, Context};

use chrono::prelude::*;

use futures::future::join_all;
use log::debug;
use rand::random;

use std::{
    cmp,
    collections::HashMap,
    marker::PhantomData,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{atomic::AtomicBool, Arc},
};

use tokio::{
    sync::{
        mpsc::{channel, Receiver, Sender},
        oneshot, Notify, RwLock,
    },
    time::{self, sleep, Duration},
};

use anyhow::Result;
use tracing::{error, info, warn};

// Messages for GameBot.
pub enum Msg {
    Start,
    Pause,
}

async fn handle(
    notify: Arc<Notify>,
    mut receiver: Receiver<Msg>,
    model: Arc<Model>,
    comm: Comm,
) {
    while let Some(msg) = receiver.recv().await {
        match msg {
            Msg::Pause => {
                model
                    .should_run
                    .store(false, std::sync::atomic::Ordering::Relaxed);
                notify.notify_waiters();
            }
            Msg::Start => {
                model
                    .should_run
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                comm.flagbot(flagbot::Msg::Start).await;
                notify.notify_waiters();
            }
        }
    }
}

pub struct Game<F, S> {
    config: Config,
    teams: TeamRegistry,
    services: ServiceRegistry,
    slot: SlotRegistry,

    current_round: RoundId,
    flag_submission_addr: SocketAddr,
    db: Db,

    flag_codec: PhantomData<F>,
    score: PhantomData<S>,
}

pub struct Model {
    should_run: AtomicBool,
    flagbot_model: Arc<flagbot::Model>,
    scorebot_model: Arc<scoring::Model>,

    pub db: Db,
    pub teams: TeamRegistry,
    pub services: ServiceRegistry,
    pub slot: SlotRegistry,
    pub service_state_map: RwLock<ServiceStateMap>,
    pub config: Config,
}

impl Model {
    pub fn scorebot_model(&self) -> Arc<scoring::Model> {
        self.scorebot_model.clone()
    }
    pub fn flagbot_model(&self) -> Arc<flagbot::Model> {
        self.flagbot_model.clone()
    }
}

impl<F: FlagCodec + Send + 'static, S: Score + Send + Clone + 'static>
    Game<F, S>
{
    pub fn new(
        config: Config,
        service_map: ServiceRegistry,
        team_map: TeamRegistry,
        slot_map: SlotRegistry,
        db: Db,
    ) -> Result<Self> {
        // Get the last finished round. If the game haven't been started yet.
        // This will return a round with round_number:0 . If not this will return the RoundId of a round
        // which was completed. We can ignore any unfinished round and continue next round.
        let current_round = journal::RoundJournal::get_last_finished_round(
            &db,
        )?
        .ok_or(anyhow!(
            "Internal Error: Unable to retrieve last_finished_round"
        ))?;
        let socket = SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)),
            config.flag_port(),
        );

        Ok(Self {
            teams: team_map,
            services: service_map,
            slot: slot_map,
            flag_codec: PhantomData,
            score: PhantomData,
            flag_submission_addr: socket,
            config,
            current_round,
            db: db.clone(),
        })
    }

    pub async fn run(self) -> anyhow::Result<()> {
        if self.current_round.round_number == 0 {
            // We are starting a new game
            info!("Starting a new game !!");
        }

        // Initialize different bots
        let mut flagbot = FlagBot::new(
            self.flag_submission_addr,
            self.teams.clone(),
            self.services.clone(),
            self.config.flag_life() as usize,
            self.current_round.clone(),
        )
        .context("Unable to initialize FlagBot")?;

        let checkerbot = checker::CheckerBot::new(
            self.services.clone(),
            self.teams.clone(),
            self.slot.clone(),
        );

        let scorebot =
            ScoreBot::<S>::new(self.teams.clone(), self.services.clone())
                .context("Unable to initialize ScoreBot")?;

        let flag_map = tables::FlagStore::all(&self.db)?;
        let mut flag_submission_set = FlagSubmissionSet::new();
        let mut service_state_map = ServiceStateMap::new();
        let rounds = journal::RoundJournal::entries(&self.db)?;

        let flag_submission_entry =
            journal::FlagSubmissionJournal::entries(&self.db)?;

        let service_state_entry =
            journal::ServiceStateJournal::entries(&self.db)?;

        // Create FlagSubmission Set which will be used to ensure that we don't accept duplicate submissions
        for entry in flag_submission_entry.iter() {
            let ret = flag_submission_set.insert((
                entry.attacker,
                entry.victim,
                entry.flag_id,
            ));

            if !ret {
                error!("Duplicate FlagSubmission Entry: {:?}", entry);
            }
        }

        // Create ServiceState Map this is the internal map used to update and retieve the state of the services
        // during the game
        for entry in service_state_entry.iter() {
            update_service_state_map(
                &mut service_state_map,
                entry.team_id,
                entry.service_id,
                entry.round_id,
                &entry.state,
            );
        }

        let service_state_map = RwLock::new(service_state_map);

        // Setup the game model
        let model = Arc::new(Model {
            should_run: AtomicBool::new(false),

            teams: self.teams.clone(),
            services: self.services.clone(),
            slot: self.slot.clone(),
            config: self.config.clone(),
            service_state_map,

            flagbot_model: flagbot.model(),
            scorebot_model: scorebot.model(),

            db: self.db.clone(),
        });

        let scorebot_sender = scorebot.sender();
        let checker_sender = checkerbot.sender();
        let flagbot_sender = flagbot.sender();

        let (sender, receiver) = channel(10);
        let comm = Comm {
            flagbot: flagbot_sender,
            checkerbot: checker_sender,
            scorebot: scorebot_sender,
            gamebot: sender,
        };

        if let Some(listener) = flagbot.event_listener() {
            tokio::spawn(handle_flagbot_event(
                model.clone(),
                comm.clone(),
                listener,
            ));
        }

        // First check if  the checker services are up
        // Only start the game once all the checker bots
        // have started
        // let result = checkerbot.is_checkers_up().await;
        // for (service_id, status) in result {
        //     if status == false {
        //         if let Some(service) = self.services.get(&service_id) {
        //             warn!(
        //                 "Unable to Connect to service<{:?}> checker @ {}:{}",
        //                 service_id,
        //                 service.checker_host(),
        //                 service.checker_port()
        //             )
        //         }
        //     }
        // }

        let buffer = checker_task_buffer(self.config.tick_time());
        info!("Make sure all the checkers are able to complete a task within: {} seconds", buffer);

        tokio::spawn(checkerbot.run::<F>());
        tokio::spawn(scorebot.run());
        tokio::spawn(flagbot.run::<F>());

        let notify = Arc::new(Notify::new());
        tokio::spawn(handle(
            notify.clone(),
            receiver,
            model.clone(),
            comm.clone(),
        ));

        // Now we replay data from Journal and tables to make the bots up to date and have valid state for submission

        // For FlagBot
        // 1. Update all the flag data.
        for (flag_id, flag) in flag_map.iter() {
            let msg = flagbot::Msg::NewFlag(*flag_id, flag.clone());
            comm.flagbot(msg).await;
        }

        // 2. Send all valid round_ids to the bot
        for (round_id, _) in rounds {
            let msg = flagbot::Msg::UpdateRound(round_id);
            comm.flagbot(msg).await;
        }

        // 3. Update all the flag submission.
        for submission in flag_submission_entry.iter() {
            let msg = flagbot::Msg::SubmittedFlag(
                submission.round_id,
                submission.attacker,
                submission.flag_id,
            );
            comm.flagbot(msg).await;
        }

        // For Scorebot
        // We have flag_submission entry service_state entry and rounds which all should be cronologicaly sorted before
        // sending to scorebot.
        let rounds = journal::RoundJournal::entries(&model.db)?;

        replay_scorebot(
            rounds,
            flag_map,
            flag_submission_entry,
            service_state_entry,
            comm.clone(),
        )
        .await;

        let progress_game = tokio::spawn(progress_game(
            comm.clone(),
            self.services.clone(),
            self.teams.clone(),
            self.slot.clone(),
            self.config.tick_time(),
            model.clone(),
            notify.clone(),
            self.current_round,
        ));

        let check_service = tokio::spawn(check_services(
            model.clone(),
            comm.clone(),
            self.config.tick_time(),
            notify.clone(),
        ));

        let game_api = tokio::spawn(game_api(
            self.teams.clone(),
            self.services.clone(),
            comm.clone(),
            model.clone(),
        ));

        // comm.flagbot(flagbot::Msg::Start).await;

        // Spawn tasks to handle errors from different tasks which handle the game
        // Ideally these tasks shouldn't return. Returning from these means that there
        // are some fatal issues
        tokio::spawn(handle_task(progress_game, "progress_game"));
        tokio::spawn(handle_task(check_service, "check_services"));
        tokio::spawn(handle_task(game_api, "game_api"));

        sleep(Duration::from_secs(10_000_000)).await;

        Ok(())
    }
}

#[derive(Clone)]
pub struct Comm {
    flagbot: Sender<flagbot::Msg>,
    checkerbot: Sender<checker::Request>,
    scorebot: Sender<scoring::Msg>,
    gamebot: Sender<Msg>,
}

impl Comm {
    pub(crate) async fn flagbot(&self, msg: flagbot::Msg) {
        if let Err(err) = self.flagbot.send(msg).await {
            error!("Unable to send Msg to flagbot : {}", err)
        }
    }
    pub(crate) async fn scorebot(&self, msg: scoring::Msg) {
        if let Err(err) = self.scorebot.send(msg).await {
            error!("Unable to send Msg to scorebot : {}", err)
        }
    }

    pub(crate) async fn checkerbot(&self, request: checker::Request) {
        if let Err(e) = self.checkerbot.send(request).await {
            error!("Unable to send Msg to checkerbot : {}", e);
        }
    }

    pub(crate) async fn gamebot(&self, request: Msg) {
        if let Err(e) = self.gamebot.send(request).await {
            error!("Unable to send Msg to checkerbot : {}", e);
        }
    }
}

async fn replay_scorebot(
    rounds: Vec<(RoundId, DateTime<Utc>)>,
    flag_map: FlagRegistry,
    flag_submission_entry: Vec<FlagSubmissionJournalEntry>,
    service_state_entry: Vec<ServiceStateJournalEntry>,
    comm: Comm,
) {
    // Some internal structures for soring scoreboard events accoding to timestamp
    enum Event {
        RoundStart(RoundId, DateTime<Utc>),
        ServiceState(ServiceStateJournalEntry),
        FlagSubmission(FlagSubmissionJournalEntry),
    }

    impl Event {
        fn timestamp(&self) -> &DateTime<Utc> {
            match self {
                Event::RoundStart(_, t) => t,
                Event::ServiceState(e) => &e.timestamp,
                Event::FlagSubmission(e) => &e.submission_time,
            }
        }
    }

    let mut all_events: Vec<Event> = Vec::new();

    // Convert all entries to Events
    all_events.extend(
        rounds
            .into_iter()
            .map(|(id, time)| Event::RoundStart(id, time)),
    );
    all_events.extend(service_state_entry.into_iter().map(Event::ServiceState));
    all_events
        .extend(flag_submission_entry.into_iter().map(Event::FlagSubmission));

    all_events.sort_by(|a, b| a.timestamp().cmp(b.timestamp()));

    for events in all_events {
        let msg = match events {
            Event::RoundStart(round_id, _) => {
                Some(scoring::Msg::UpdateRound(round_id))
            }

            Event::FlagSubmission(submission) => {
                flag_map.get(&submission.flag_id).map(|flag| {
                    scoring::Msg::FlagSubmission(
                        submission.attacker,
                        FlagKey::from(flag),
                    )
                })
            }

            Event::ServiceState(entry) => {
                Some(scoring::Msg::ServiceStateUpdated(
                    entry.team_id,
                    entry.service_id,
                    entry.round_id,
                    entry.state,
                ))
            }
        };

        if let Some(msg) = msg {
            comm.scorebot(msg).await;
        }
    }
}

/// Handle flagbot event. This includes flag submission.
async fn handle_flagbot_event(
    model: Arc<Model>,
    comm: Comm,
    mut listener: Receiver<flagbot::FlagEvent>,
) -> Result<()> {
    while let Some(event) = listener.recv().await {
        match event {
            FlagEvent::SubmittedFlag(round_id, attacker, flag_id) => {
                let flag = tables::FlagStore::get(&model.db, flag_id)?;
                let flag_key = FlagKey::from(&flag);
                let msg = scoring::Msg::FlagSubmission(attacker, flag_key);

                // Update Journal with flag submissions
                let entry = FlagSubmissionJournalEntry {
                    attacker,
                    flag_id,
                    round_id,
                    victim: Victim::new(flag.team_id()),
                    submission_time: Utc::now(),
                };
                journal::FlagSubmissionJournal::append(&model.db, entry)?;

                // Notify the scorebot about receiving new flag from a team.
                comm.scorebot(msg).await;
            }
        }
    }

    Ok(())
}

fn should_run(model: &Model) -> bool {
    model.should_run.load(std::sync::atomic::Ordering::Relaxed)
}

/// Wait for round_duration then start a new round.
async fn progress_game(
    comm: Comm,
    services: ServiceRegistry,
    teams: TeamRegistry,
    slot: SlotRegistry,
    round_duration: u64,
    model: Arc<Model>,
    notify: Arc<Notify>,
    current_round: RoundId,
) -> anyhow::Result<()> {
    let mut stopped_once = false;
    let mut round_id = current_round;

    loop {
        // Sleep for 100ms and check if you should start the game
        sleep(Duration::from_millis(100)).await;
        if !should_run(&model) {
            // Stop flagbot also since this is the end of a round
            comm.flagbot(flagbot::Msg::Pause).await;
            info!("Stopped Game after round : {}", round_id.round_number);
            info!("Send START command to start the Game");

            // Mark the previous round as finished
            journal::RoundJournal::mark_round_as_finished(&model.db, round_id)?;

            // The state of the Game has changed. Check Again
            notify.notified().await;

            continue;
        }

        // Pause the game when we finish final round.
        if (round_id.round_number as u64) == model.config.total_round()
            && !stopped_once
        {
            model
                .should_run
                .store(false, std::sync::atomic::Ordering::Relaxed);

            // Only stop the game once when it reaches the total_round. If the admin starts the game again.
            // continue it.
            stopped_once = true;
            continue;
        }

        let start_time = Utc::now();

        // Mark the previous round as finished
        journal::RoundJournal::mark_round_as_finished(&model.db, round_id)?;

        round_id = journal::RoundJournal::start_new_round(&model.db)?;

        info!("Starting New Round : {}", round_id.round_number);

        comm.scorebot(scoring::Msg::UpdateRound(round_id)).await;
        comm.flagbot(flagbot::Msg::UpdateRound(round_id)).await;

        let mut flag_ids = Vec::new();

        for team_id in teams.ids() {
            for service_id in services.ids() {
                let service_id = service_id;

                {
                    // Update the state of the service as Unknown for the current round.
                    let service_state = ServiceState::Unknown;
                    journal::ServiceStateJournal::append(
                        &model.db,
                        round_id,
                        team_id,
                        service_id,
                        service_state,
                        journal::ServiceStateJournalType::Initial,
                    )?;
                }

                for (slot, s) in slot.inner().iter() {
                    // Initially the state of the service is set as Unknown.
                    // Generate new flag for all the teams and notify checker to plant those
                    // flags.

                    if service_id != *s {
                        continue;
                    }

                    let flag = Flag::new(round_id, *slot, service_id, team_id);
                    let flag_id = tables::FlagStore::insert(&model.db, &flag)?;

                    flag_ids.push(flag_id);

                    let (sender, receiver) = oneshot::channel();
                    let message =
                        checker::RequestMsg::PlantFlag(flag_id, flag.clone());
                    let request = (sender, message.clone());

                    comm.checkerbot(request).await;
                    tokio::spawn(handle_checkerbot_response(
                        model.clone(),
                        comm.clone(),
                        receiver,
                        message,
                    ));

                    let request = flagbot::Msg::NewFlag(flag_id, flag);
                    comm.flagbot(request).await;
                }
            }
        }

        // Start checking service for this round.
        tokio::spawn(check_services_for_round(
            round_id,
            model.clone(),
            comm.clone(),
            round_duration,
        ));

        let end_time = Utc::now();
        let delta = (end_time - start_time).num_milliseconds();
        let sleep_duration = round_duration as i64 * 1000 - delta;
        if sleep_duration > 0 {
            sleep(Duration::from_millis(sleep_duration as u64)).await;
        }
    }
}

// Buffer is minimum amount of time required for the checker to complete its task.
fn checker_task_buffer(round_duration: u64) -> u64 {
    (round_duration as f64 * 0.20).ceil() as u64
}

// Periodically check for the status of the service.
async fn check_services(
    model: Arc<Model>,
    comm: Comm,
    round_duration: u64,
    notify: Arc<Notify>,
) -> Result<()> {
    // Buffer is minimum amount of time required for the checker to complete its task.
    let buffer = checker_task_buffer(round_duration);

    loop {
        if !should_run(&model) {
            // The state of the Game has changed. Check Again
            notify.notified().await;
            continue;
        }

        let jitter = rand::random::<u64>() % (round_duration - buffer);
        time::sleep(Duration::from_secs(jitter)).await;

        let current_round = journal::RoundJournal::get_current(&model.db)?;

        if current_round.round_number == 0 {
            continue;
        }

        info!(
            "Randomly Checking Service for Round<{}>",
            current_round.round_number
        );
        if let Err(e) =
            check_service(model.clone(), &comm, current_round.clone()).await
        {
            error!("Unable to CheckService : {}", e);
        }

        let remaining = round_duration - jitter;
        time::sleep(Duration::from_secs(remaining)).await;
    }
}

// Periodically check for the status of the service.
async fn check_services_for_round(
    current_round: RoundId,
    model: Arc<Model>,
    comm: Comm,
    round_duration: u64,
) -> Result<()> {
    // Buffer is minimum amount of time required for the checker to complete its task.
    let buffer = checker_task_buffer(round_duration);

    // Since we need spacing at the start and end, removing buffer * 2 from time_left.
    //
    let mut time_left = round_duration - (buffer * 2);

    // Need to change the constants if the number of checks during the round is changed
    // Currently we are doing 2 checks in a round.

    let mut check_flag = rand::random::<bool>();

    for _ in 0..2 {
        if current_round.is_round_zero() {
            continue;
        }

        // Add a jitter.
        let jitter = random::<u64>() % (time_left / 2);
        time::sleep(Duration::from_secs(buffer)).await;
        time::sleep(Duration::from_secs(jitter)).await;

        time_left = time_left - jitter;

        if check_flag {
            // Check flags for all the teams.
            info!("Checking Flags for Round<{}>", current_round.round_number);
            for team_id in model.teams.ids() {
                for service_id in model.services.ids() {
                    tokio::spawn(check_flags(
                        model.clone(),
                        comm.clone(),
                        team_id,
                        service_id,
                        current_round,
                    ));
                }
            }
            check_flag = false;
        } else {
            info!("Checking Service for Round<{}>", current_round.round_number);
            if let Err(e) =
                check_service(model.clone(), &comm, current_round.clone()).await
            {
                error!("Unable to CheckService : {}", e);
            }

            check_flag = true;
        }
    }

    Ok(())
}

// Generic function which handles the response from checkerbot.
async fn handle_checkerbot_response(
    model: Arc<Model>,
    comm: Comm,
    receiver: oneshot::Receiver<checker::ResponseMsg>,
    request: checker::RequestMsg,
) -> Result<()> {
    if let Ok(response) = receiver.await {
        match response {
            ResponseMsg::Failed(error) => {
                error!(
                    "CheckerBot Failed for request {:?} : {:?}",
                    request, error
                );
            }

            ResponseMsg::PlantedFlag(
                round_id,
                slot_id,
                team_id,
                service_state,
                flag_id,
                token,
                identifier,
            ) => {
                if let Some(service_id) = model.slot.get(&slot_id).cloned() {
                    tables::FlagStore::update_token(&model.db, flag_id, token)?;
                    tables::FlagStore::update_identifier(
                        &model.db, flag_id, identifier,
                    )?;

                    update_service_state(
                        model.clone(),
                        &comm,
                        round_id,
                        team_id,
                        service_id,
                        service_state,
                        ServiceStateJournalType::PlantFlag,
                    )
                    .await?;
                }
            }
            ResponseMsg::ServiceState(
                _round_id,
                service_id,
                team_id,
                service_state,
            ) => {
                // Update journal entry with update in service state.
                let current_round =
                    journal::RoundJournal::get_current(&model.db)?;

                update_service_state(
                    model.clone(),
                    &comm,
                    current_round,
                    team_id,
                    service_id,
                    service_state,
                    ServiceStateJournalType::ServiceCheck,
                )
                .await?;
            }
        }
    }
    Ok(())
}

async fn check_flags(
    model: Arc<Model>,
    comm: Comm,

    team_id: TeamId,
    service_id: ServiceId,
    current_round: RoundId,
) -> Result<()> {
    // Check service one by one

    // Retrive the current rounds service state
    let key = ServiceStateKey {
        team_id,
        round_id: current_round,
        service_id,
    };

    let state = {
        model
            .service_state_map
            .read()
            .await
            .get(&key)
            .ok_or(anyhow!(
                "Unable to find ServiceState in map for key: {:?}",
                key
            ))?
            .to_owned()
    };

    // Don't check if the service is not up.
    if state != ServiceState::Up {
        return Ok(());
    }

    // Check if the service have flags for previous rounds
    let mut rounds = journal::RoundJournal::get_previous_n_finished(
        &model.db,
        model.config.flag_life() as usize,
    )?;

    rounds.reverse();

    // Remove Round 0 if exist in the pool
    rounds.retain(|round| !round.is_round_zero());

    // No of rounds to check
    if rounds.len() == 0 {
        return Ok(());
    }

    // Collect all the flags
    let flags: Vec<(FlagId, Flag)> = {
        let mut collected_flags = Vec::new();
        for round in &rounds {
            if let Ok(flags) = FlagStore::get_by_round(&model.db, *round) {
                for (flag_id, flag) in flags {
                    if flag.team_id() == team_id
                        && flag.service_id() == service_id
                    {
                        collected_flags.push((flag_id, flag));
                    }
                }
            }
        }
        collected_flags
    };

    let mut receivers = Vec::with_capacity(flags.len());
    // Requeset checker to check all the flags
    for (_flag_id, flag) in flags {
        let token = flag.token();
        if token.is_none() {
            continue;
        }
        let (sender, receiver) = oneshot::channel();
        let message =
            checker::RequestMsg::CheckFlag(flag.clone(), token.clone());
        let request = (sender, message.clone());
        comm.checkerbot(request).await;
        let fut = async { (message, receiver.await) };
        receivers.push(fut);
    }

    // Initialize service state map
    let mut status: HashMap<RoundId, ServiceState> = HashMap::new();
    for round in &rounds {
        status.insert(*round, ServiceState::Unknown);
    }

    let responses = join_all(receivers).await;

    // Note: We are ignoreing oneshot recv error !!
    // Filter all valid responses
    let responses: Vec<(checker::RequestMsg, checker::ResponseMsg)> = responses
        .into_iter()
        .filter_map(|(message, response)| {
            if let Ok(response) = response {
                Some((message, response))
            } else {
                None
            }
        })
        .collect();

    for (request, response) in responses {
        match response {
            ResponseMsg::Failed(msg) => {
                // TODO: attach request information also.
                error!(
                    "CheckerBot Failed for request {:?} : {:?}",
                    request, msg
                );
            }
            ResponseMsg::PlantedFlag(_, _, _, _, _, _, _) => {
                error!("CheckerBot returned PlantedFlag reponse for CheckFlag request!!!!!")
            }
            ResponseMsg::ServiceState(
                round_id,
                r_service_id,
                r_team_id,
                service_state,
            ) => {
                // We should only receive response for a specific team and service
                assert_eq!(service_id, r_service_id);
                assert_eq!(team_id, r_team_id);

                status
                    .entry(round_id)
                    .and_modify(|previous_state| {
                        if crate::service::should_update(
                            &*previous_state,
                            &service_state,
                        ) {
                            *previous_state = service_state.clone()
                        }
                    })
                    .or_insert(service_state);
            }
        }
    }

    // Got the flag status of last round.
    let last_flag_status = rounds
        .pop()
        .map(|last_round| status.remove_entry(&last_round))
        .flatten()
        .map(|(_x, y)| y);

    let final_status;
    match last_flag_status {
        Some(ServiceState::Up) | None => {
            let total_round = rounds.len();

            // We should have status of all the rounds here.
            assert_eq!(total_round, status.len());

            let up_rounds = status
                .values()
                .into_iter()
                .filter(|status| **status == ServiceState::Up)
                .count();

            debug!(
                "Check Flag {:?} {:?} RoundId<{}> : {} flags a valid",
                team_id, service_id, current_round.round_number, up_rounds
            );

            if up_rounds
                >= cmp::min(
                    total_round,
                    f64::ceil(
                        model.config.flag_life().saturating_sub(1) as f64
                            * 0.80,
                    ) as usize,
                )
            {
                final_status = ServiceState::Up
            } else {
                final_status = ServiceState::Recovering
            }
        }
        Some(status) => {
            final_status = status;
        }
    }

    update_service_state(
        model,
        &comm,
        current_round,
        team_id,
        service_id,
        final_status,
        ServiceStateJournalType::CheckFlag,
    )
    .await?;

    Ok(())
}

async fn check_service(
    model: Arc<Model>,
    comm: &Comm,
    current_round: RoundId,
) -> Result<()> {
    let teams = model.teams.clone();
    let services = model.services.clone();
    for team in teams.ids() {
        for service in services.ids() {
            let model = model.clone();

            // TODO: This query is expensive
            let state = journal::ServiceStateJournal::get(
                &model.db,
                current_round,
                team,
                service,
            )?;

            if let ServiceState::Up | ServiceState::Mumble(_) = state {
                let round_id = current_round;
                let service_id = service;
                let team_id = team;

                let (sender, receiver) = oneshot::channel();
                let msg = checker::RequestMsg::CheckService(
                    team_id, service_id, round_id,
                );
                let request = (sender, msg.clone());
                comm.checkerbot(request).await;
                tokio::spawn(handle_checkerbot_response(
                    model.clone(),
                    comm.clone(),
                    receiver,
                    msg,
                ));
            }
        }
    }

    Ok(())
}

async fn handle_task<T>(
    task: tokio::task::JoinHandle<Result<T>>,
    task_name: &str,
) {
    match task.await {
        Ok(Ok(_)) => {}
        Ok(Err(err)) => error!("{} task failed: {:?}", task_name, err),
        Err(err) => error!("{} task join error: {:?}", task_name, err),
    }
}

async fn update_service_state(
    model: Arc<Model>,
    comm: &Comm,
    round_id: RoundId,
    team_id: TeamId,
    service_id: ServiceId,
    service_state: ServiceState,
    state_type: ServiceStateJournalType,
) -> anyhow::Result<()> {
    journal::ServiceStateJournal::append(
        &model.db,
        round_id,
        team_id,
        service_id,
        service_state.clone(),
        state_type,
    )?;

    {
        let mut map = model.service_state_map.write().await;
        update_service_state_map(
            &mut map,
            team_id,
            service_id,
            round_id,
            &service_state,
        );
    }

    let msg = scoring::Msg::ServiceStateUpdated(
        team_id,
        service_id,
        round_id,
        service_state,
    );
    comm.scorebot(msg).await;

    Ok(())
}

fn update_service_state_map(
    map: &mut ServiceStateMap,
    team_id: TeamId,
    service_id: ServiceId,
    round_id: RoundId,
    service_state: &ServiceState,
) {
    let key = ServiceStateKey {
        round_id,
        team_id,
        service_id,
    };

    map.entry(key)
        .and_modify(|value| {
            if crate::service::should_update(&*value, service_state) {
                *value = service_state.clone()
            }
        })
        .or_insert(service_state.clone());
}
