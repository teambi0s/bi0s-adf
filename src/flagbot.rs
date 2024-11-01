use std::{
    collections::HashMap,
    error::Error,
    hash::Hash,
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter},
    net::{TcpListener, TcpStream},
    sync::{
        mpsc::{self, channel, Receiver, Sender},
        Notify, RwLock, RwLockReadGuard, Semaphore,
    },
    time::sleep,
};

use anyhow::{Context, Result};
use tracing::{debug, error, info, warn};

use crate::{
    db::{
        journal::RoundId,
        tables::{FlagId, ServiceId, TeamId},
    },
    flag::{Flag, FlagCodec, FlagRegistry, FlagSecret},
    history::History,
    service::ServiceRegistry,
    teams::{Attacker, TeamRegistry},
};

#[derive(PartialEq, Hash, Debug, Eq)]
struct Submitter(Attacker, FlagId);

pub struct Model {
    running: AtomicBool,
    throttle_submission: AtomicBool,

    current_round: RwLock<RoundId>,
    flag_map: RwLock<FlagRegistry>,
    secret_map: RwLock<HashMap<FlagSecret, FlagId>>,
    flag_submission: RwLock<HashMap<Submitter, bool>>,
    tcp_config: RwLock<TcpConfig>,

    // These internal data structures can be accessed publicly.
    // We only give read access so that the state of the flag bot can only
    // be modified by sending messages.
    h_flag_gained: RwLock<History<u64>>,
    h_flag_lost: RwLock<History<u64>>,
    metrics: RwLock<Metrics>,
}

impl Model {
    fn new(teams: &TeamRegistry, current_round: RoundId) -> Self {
        let tcp_config = TcpConfig::default();

        let mut metrics = Metrics::default();
        for team in teams.ids() {
            metrics.insert(team, FlagSubmissionMetrics::default());
        }

        let h_flag_gained = RwLock::new(History::new());
        let h_flag_lost = RwLock::new(History::new());

        Self {
            current_round: RwLock::new(current_round),
            running: AtomicBool::new(true),
            throttle_submission: AtomicBool::new(false),
            flag_map: RwLock::new(HashMap::new()),
            secret_map: RwLock::new(HashMap::new()),
            flag_submission: RwLock::new(HashMap::new()),
            metrics: RwLock::new(metrics),
            tcp_config: RwLock::new(tcp_config),
            h_flag_gained,
            h_flag_lost,
        }
    }

    pub async fn h_flag_gained(&self) -> RwLockReadGuard<'_, History<u64>> {
        self.h_flag_gained.read().await
    }

    pub async fn h_flag_lost(&self) -> RwLockReadGuard<'_, History<u64>> {
        self.h_flag_lost.read().await
    }

    pub async fn flag_map(&self) -> RwLockReadGuard<'_, FlagRegistry> {
        self.flag_map.read().await
    }

    pub async fn metrics(&self) -> RwLockReadGuard<'_, Metrics> {
        self.metrics.read().await
    }
}

// Response Message from flagbot
#[derive(Debug)]
pub enum FlagEvent {
    // Notify others that an attacker has submitted a flag and the flag has been validated.
    SubmittedFlag(RoundId, Attacker, FlagId),
}

// Messages which flagbot can handle
#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum Msg {
    // Start Accepting Flags
    Start,
    // Stop Accepting Flags
    Pause,
    // Start Throttling Flag Submission
    ThrottleSubmission(bool),
    // Update TcpConfig
    UpdateTcpConfig(TcpConfig),

    // These messages update the internal state of the bot
    // While replying these messages are used to make the bot up to date

    // Update the current Round
    UpdateRound(RoundId),
    // Receive new Flag
    NewFlag(FlagId, Flag),
    // Reply flagevent back to Flagbot
    SubmittedFlag(RoundId, Attacker, FlagId),
}

impl Msg {
    async fn handle(
        teams: TeamRegistry,
        services: ServiceRegistry,
        mut receiver: Receiver<Msg>,
        notify: Arc<Notify>,
        model: Arc<Model>,
    ) {
        while let Some(msg) = receiver.recv().await {
            debug!("FlagBot Msg handler : {:?}", &msg);
            match msg {
                Msg::UpdateRound(round) => {
                    // Update Current Round for Flagbot.
                    let mut current_round = model.current_round.write().await;
                    *current_round = round;

                    // Initialize Flag lost/gain history map for this current round.
                    {
                        // Don't initialize flag stores for round 0
                        if current_round.is_round_zero() {
                            continue;
                        }

                        let mut h_flag_gained =
                            model.h_flag_gained.write().await;
                        let mut h_flag_lost = model.h_flag_lost.write().await;

                        for team in teams.ids() {
                            for service in services.ids() {
                                h_flag_lost.insert(team, service, round, 0);
                                h_flag_gained.insert(team, service, round, 0);
                            }
                        }
                    }
                }

                Msg::Start => {
                    // Start accepting flags.
                    model.running.store(true, Ordering::SeqCst);
                    notify.notify_waiters();
                }

                Msg::Pause => {
                    // Stop accepting flags.
                    model.running.store(false, Ordering::SeqCst);
                    notify.notify_waiters();
                }
                Msg::ThrottleSubmission(status) => {
                    // Whether to throttle flag submission for players.
                    model.throttle_submission.store(status, Ordering::SeqCst)
                }
                Msg::NewFlag(flag_id, flag) => {
                    // Update internal maps with a new flag.
                    let flag_map = &model.flag_map;
                    let secret_map = &model.secret_map;
                    let flag_submission = &model.flag_submission;

                    let mut flag_map = flag_map.write().await;
                    let mut secret_map = secret_map.write().await;
                    let mut flag_submission = flag_submission.write().await;

                    for team in teams.ids() {
                        let submitter =
                            Submitter(Attacker::new(team.clone()), flag_id);
                        if flag_submission.insert(submitter, false).is_some() {
                            error!("Added duplicate submission to flag_submission model !!");
                        }
                    }

                    secret_map.insert(flag.secret().clone(), flag_id);
                    flag_map.insert(flag_id, flag);
                }
                Msg::UpdateTcpConfig(tcp_config) => {
                    // Update TcpConfig which will be used to throttle flag submission from the players.
                    let mut config_store = model.tcp_config.write().await;
                    *config_store = tcp_config;
                }

                Msg::SubmittedFlag(round_id, attacker, flag_id) => {
                    // Updated the internal maps with the submission details.
                    let flag_submission = &model.flag_submission;
                    let mut flag_submission = flag_submission.write().await;
                    let submitter = Submitter(attacker.clone(), flag_id);

                    // Mark the flag as submitted for a particular user.
                    flag_submission.insert(submitter, true);

                    {
                        // Retrieve flag structure from the internal data structure.
                        if let Some(flag) =
                            model.flag_map.read().await.get(&flag_id)
                        {
                            let service_id = flag.service_id();

                            let mut h_flag_gained =
                                model.h_flag_gained.write().await;
                            let mut h_flag_lost =
                                model.h_flag_lost.write().await;

                            // Mark that attacker has gained a flag in this current round.
                            if let Some(value) = h_flag_gained.get(
                                attacker.id(),
                                service_id,
                                round_id,
                            ) {
                                let value = value + 1;
                                h_flag_gained.insert(
                                    attacker.id(),
                                    service_id,
                                    round_id,
                                    value,
                                );
                            }

                            // Mark that victim has lost a flag in this current round.
                            if let Some(value) = h_flag_lost.get(
                                flag.team_id(),
                                service_id,
                                round_id,
                            ) {
                                let value = value + 1;
                                h_flag_lost.insert(
                                    flag.team_id(),
                                    service_id,
                                    round_id,
                                    value,
                                );
                            }
                        }
                    }
                }
            }
        }
        error!("Exiting FlagBot");
    }
}

// By default we create TCP option for flag submission.
// We assume the flags for TCP are newline separated
pub struct FlagBot {
    // Address on which we should listen for flags
    tcp_addr: SocketAddr,
    // Channel used for passing flag event.
    event_sender: Sender<FlagEvent>,
    event_listener: Option<Receiver<FlagEvent>>,
    // Communication channel used to pass messages.
    comm: (Sender<Msg>, Receiver<Msg>),
    // List of teams
    teams: TeamRegistry,
    // List of services
    services: ServiceRegistry,
    // How many rounds should the flag be kept alive
    flag_life: usize,

    // All data stored for processing in FlagBot
    model: Arc<Model>,
}

impl FlagBot {
    pub fn new(
        addr: SocketAddr,
        teams: TeamRegistry,
        services: ServiceRegistry,
        flag_life: usize,
        current_round: RoundId,
    ) -> Result<Self> {
        let event_channel = channel(10);
        let comm = mpsc::channel(10);
        let model = Arc::new(Model::new(&teams, current_round));

        Ok(FlagBot {
            tcp_addr: addr,
            event_sender: event_channel.0,
            event_listener: Some(event_channel.1),
            comm,
            teams,
            services,
            flag_life,
            model,
        })
    }

    pub fn sender(&self) -> Sender<Msg> {
        self.comm.0.clone()
    }

    pub fn event_listener(&mut self) -> Option<Receiver<FlagEvent>> {
        self.event_listener.take()
    }

    pub fn model(&self) -> Arc<Model> {
        self.model.clone()
    }

    pub async fn run<C: FlagCodec + Send>(self) -> Result<()> {
        info!("Starting FlagBot");

        let notify = Arc::new(Notify::new());
        let n1 = notify.clone();

        let sender = self.sender();

        // Handle Message for flagbot.
        tokio::spawn(Msg::handle(
            self.teams.clone(),
            self.services.clone(),
            self.comm.1,
            n1,
            self.model.clone(),
        ));

        // tokio::spawn(print_stats(self.model.clone()));

        loop {
            // Only start running flagbot if running state is set to true.
            notify.notified().await;
            if !should_run(&self.model) {
                continue;
            }

            // Creates TCP endpoint for incoming flag submissions.
            if let Err(e) = create_tcp_endpoint::<C>(
                &self.teams,
                self.tcp_addr,
                self.model.clone(),
                self.event_sender.clone(),
                sender.clone(),
                self.flag_life,
                notify.clone(),
            )
            .await
            {
                error!("TCP endpoint exited with : {}", e);
            }
        }
    }
}

async fn create_tcp_endpoint<C: FlagCodec + Send>(
    teams: &TeamRegistry,
    endpoint: SocketAddr,
    model: Arc<Model>,
    flag_event: Sender<FlagEvent>,
    flagbot_msg_sender: Sender<Msg>,
    flag_life: usize,
    notify: Arc<Notify>,
) -> Result<()> {
    let config = model.tcp_config.read().await.clone();

    // Create a mapping between IP of the team and team_id
    // We will be authenticating teams with their IP.
    let mut ip_team_map = HashMap::new();
    for (team_id, team) in teams.inner().iter() {
        ip_team_map.insert(team.ip(), *team_id);
    }

    // Create a semaphore which limits the amount of maximum connections created from a team.
    let mut max_connection_map = HashMap::new();
    for team_id in teams.ids() {
        max_connection_map.insert(
            team_id,
            Arc::new(Semaphore::new(config.max_connection as usize)),
        );
    }

    // Listen for incoming TCP connections.
    let listener = TcpListener::bind(endpoint)
        .await
        .context("Binding to TCP for incoming flag submissions")?;

    info!("Started TCP endpoint for flag submission : {}", endpoint);
    loop {
        tokio::select! {
            // Manages the state of the endpoint, closes the endpoint if the running state has been changed to false.
            _ = notify.notified() => {
                if !should_run(&model){
                    info!("Stopping TCP endpoint : {}", endpoint);
                    return Ok(())
                }
            }

            res = listener.accept() => {
                match res {
                    Ok((mut socket, addr)) => {
                        debug!("Got a flag_submission connection from team: {}", addr);
                        let team_id = ip_team_map.get(&addr.ip());
                        match team_id {
                            Some(id) => {
                                // Update TCP connection metrics
                                {

                                    if let Some(metrics) = model.metrics.write().await.get_mut(id) {
                                        metrics.tcp_connections += 1;
                                    }
                                }

                                let semaphore = max_connection_map.get(id).unwrap();
                                let semaphore = semaphore.clone();
                                if semaphore.available_permits() == 0 {
                                    // We have exhausted the limit of connections a team is permitted to create, so notify the team and
                                    // close the connection.
                                    tokio::spawn(async move {
                                        if socket.write("Too many connections from the team\n".as_bytes()).await.map_err(|x| {
                                                error!("Error on writing 'Too many connections from team' to Socket : {}", x);
                                                x
                                            }).is_ok() {
                                                let _ = socket.shutdown().await.map_err(|x| {
                                                    error!("Unable to shut down socket : {}", x);
                                                    x
                                                });
                                            }
                                    });
                                }
                                else {
                                    let team_id  = *id;
                                    let sender = flag_event.clone();
                                    let model = model.clone();
                                    let flag_life = flag_life;
                                    let attacker = Attacker::new(team_id.to_owned());
                                    let config = config.clone();
                                    let flag_msg_sender = flagbot_msg_sender.clone();
                                    // Create a new thread for handling this connection.
                                    tokio::spawn(async move {
                                        let _permit = semaphore.acquire().await.map_err(|err| {
                                            error!("Unable to acquire Semaphore for team<{}> : {}", addr.ip(), err)
                                        });

                                        let timeout = Duration::from_secs(30);

                                        if let Err(e) = tokio::time::timeout(timeout, handle_tcp_connection::<C>(
                                            socket, model, config, attacker, flag_life, sender, flag_msg_sender,
                                        team_id.to_owned()
                                        ))
                                        .await
                                        {
                                            warn!("Handle Connection Error : {}", e);
                                        }
                                    });
                            }
                        }
                            None => {
                                // Unable to find a mapping for IP to a team. So closing the connection.
                                tokio::spawn(async move {
                                    if socket.write("Invalid Src IP\n".as_bytes()).await.map_err(|x| {
                                            error!("Error on writing 'Invalid Src IP' to Socket : {}", x);
                                            x
                                        }).is_ok() {
                                            let _ = socket.shutdown().await.map_err(|x| {
                                                error!("Unable to shut down socket : {}", x);
                                                x
                                            });
                                        }
                                });
                            }
                        }
                    }
                    Err(e) => {
                        error!(
                            "Error on accept() on TCP flag submission socket : {}",
                            e
                        )
                    }
                }
            }
        }
    }
}

#[derive(Debug)]
enum FlagStatus {
    Accepted,
    InvalidFlag,
    DuplicateSubmission,
    FlagExpired,
    SelfFlag,
}

async fn check_flag(
    model: Arc<Model>,
    subscriber: &Sender<FlagEvent>,
    sender: &Sender<Msg>,
    attacker: &Attacker,
    flag_life: usize,
    secret: &FlagSecret,
) -> Result<FlagStatus> {
    let status;
    let flag_id: FlagId = {
        let secret_map = &model.secret_map;
        let flag_id = secret_map
            .read()
            .await
            .get(secret)
            .ok_or(anyhow!(
                "Unable to get FlagId from FlagSecret, Probably Invalid Flag"
            ))?
            .clone();
        flag_id
    };

    // Retrieve the flag structure from the flag map using the flag_id.
    let flag: Flag = {
        let flag_map = &model.flag_map;
        let flag = flag_map.read().await;
        let flag = flag
            .get(&flag_id)
            .ok_or(anyhow!("Unable to get Flag from flag_map"))?;
        flag.clone()
    };

    let current_round = *model.current_round.read().await;
    let round_id = flag.round_id();
    let victim = flag.team_id();

    if victim == attacker.id() {
        status = FlagStatus::SelfFlag;
    } else if current_round < round_id {
        // This is an invalid state. This should be caught before
        // the next branch executes. Since it might cause panic due to
        // underflow of subtraction. In this case, the flag will be
        // set to an invalid state.
        error!(
            "Got Invalid Flag {:?} for Round: {:?}",
            flag_id, current_round
        );
        status = FlagStatus::InvalidFlag;
    } else if ((current_round.round_number - round_id.round_number) as usize)
        >= flag_life
    {
        status = FlagStatus::FlagExpired;
    } else {
        // TODO: Potential race condition here while submitting flags from two connections.
        let flag_submission = {
            let submission = &model.flag_submission;
            let submission = submission.read().await;
            let submitter = Submitter(attacker.clone(), flag_id);
            let submission = submission.get(&submitter).ok_or(anyhow!(
                "Unable to retrieve Submission flag from the Map"
            ))?;
            submission.clone()
        };

        if flag_submission {
            status = FlagStatus::DuplicateSubmission
        } else {
            // Modify the internal state of the flag and mark the flag as submitted.
            status = FlagStatus::Accepted;

            if let Err(_e) = sender
                .send(Msg::SubmittedFlag(
                    current_round,
                    attacker.clone(),
                    flag_id.clone(),
                ))
                .await
            {
                error!("Unable to send Flag Msg")
            }

            if let Err(e) = subscriber
                .send(FlagEvent::SubmittedFlag(
                    current_round,
                    attacker.clone(),
                    flag_id.clone(),
                ))
                .await
            {
                error!("Unable to send FlagEvent : {}", e)
            }
        }
    }
    Ok(status)
}

// Handle incoming connection and redirect the flag request to appropriate team process.
async fn handle_tcp_connection<C: FlagCodec>(
    mut stream: TcpStream,
    model: Arc<Model>,
    config: TcpConfig,
    attacker: Attacker,
    flag_life: usize,
    sender: Sender<FlagEvent>,
    flag_msg_sender: Sender<Msg>,
    team_id: TeamId,
) -> Result<(), Box<dyn Error>> {
    // let mut counter: u64 = 0;
    let mut metrics = FlagSubmissionMetrics::default();
    let (rd, wr) = stream.split();
    let mut reader = BufReader::new(rd);
    // We start the writer with lower capacity then increment the capacity after N number of flags has been received.
    let mut writer = BufWriter::with_capacity(10, wr);

    // Channel for returning the status of the flag
    let mut flag = String::with_capacity(32);
    loop {
        if model.throttle_submission.load(Ordering::SeqCst) {
            // Close Connection if the submission has hit some threshold.
            if metrics.invalid_flag == config.invalid_flags_limit {
                writer
                    .write(
                        "Closing Connection Due to large number of invalid flags\n"
                            .as_bytes(),
                    )
                    .await?;
                break;
            } else if metrics.duplicate_submission
                >= config.duplicate_submission_limit
            {
                writer
                    .write(
                        "Closing Connection due to large number of Duplicate flags\n\
                              Try optimizing your exploit.\n"
                            .as_bytes(),
                    )
                    .await?;
                break;
            } else if metrics.flag_expired >= config.expired_flags_limit {
                writer
                    .write(
                        "Closing Connection due to large number of Expired flags\n\
                               Try optimizing your exploit.\n"
                            .as_bytes(),
                    )
                    .await?;
                break;
            } else if metrics.self_flag >= config.self_flags_limit {
                writer
                    .write("Try optimizing your exploit.\n".as_bytes())
                    .await?;
                break;
            }
        }

        flag.clear();
        let res = reader.read_line(&mut flag).await;

        match res {
            Err(_) | Ok(0) => break,
            _ => {}
        }
        // remove newline
        flag.pop();
        let secret = match C::decode(&flag.as_bytes()) {
            Some(secret) => secret,
            None => {
                metrics.update(&FlagStatus::InvalidFlag);
                writer
                    .write(
                        format!(
                            "[Err] Invalid Flag - {}\n",
                            metrics.invalid_flag,
                        )
                        .as_bytes(),
                    )
                    .await?;
                continue;
            }
        };

        // We have extracted the FlagSecret, check if the flag is valid and return the status of the submission.
        let flag_status = match check_flag(
            model.clone(),
            &sender,
            &flag_msg_sender,
            &attacker,
            flag_life,
            &secret,
        )
        .await
        {
            Ok(flag_status) => flag_status,
            Err(_err) => {
                // warn!("CheckFlag Error for : {:?} : {:?}", attacker, err);
                FlagStatus::InvalidFlag
            }
        };
        metrics.update(&flag_status);

        // TODO: ???
        writer.write(format!("{} - ", &flag).as_bytes()).await?;

        match flag_status {
            FlagStatus::Accepted => {
                writer.write("[Ok]  Flag Accepted\n".as_bytes()).await?;
            }
            FlagStatus::DuplicateSubmission => {
                writer
                    .write("[Ok]  Flag Already Submitted\n".as_bytes())
                    .await?;
            }
            FlagStatus::FlagExpired => {
                writer.write("[Ok]  Flag Expired\n".as_bytes()).await?;
            }
            FlagStatus::InvalidFlag => {
                writer
                    .write(
                        format!(
                            "[Err] Invalid Flag - {} \n",
                            metrics.invalid_flag
                        )
                        .as_bytes(),
                    )
                    .await?;
            }
            FlagStatus::SelfFlag => {
                writer
                    .write("[Err] Self Flag Submitted\n".as_bytes())
                    .await?;
            }
        }

        // Increase the buffer size of writer after 100 flags. For initial flag submission we need an un-buffered writer.
        // Since we need to give the players instant feedback. If they are submitting a large number of flags we can assume that
        // it's automated and buffering can be turned on.
        if metrics.total() == config.non_buffering_limit {
            writer.flush().await?;
            writer = BufWriter::new(writer.into_inner());
        }
    }

    // Update FlagSubmission metrics information for the team.
    {
        let mut m = model.metrics.write().await;
        if let Some(team_metrics) = m.get_mut(&team_id) {
            team_metrics.append(&metrics);
        }
    }

    writer.flush().await?;
    sleep(Duration::from_secs(1)).await;
    writer.shutdown().await?;
    Ok(())
}

fn should_run(model: &Model) -> bool {
    model.running.load(Ordering::Relaxed)
}

// This structure contains limits which are used to throttle flag
// submission from a specific team.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TcpConfig {
    // Number of flags to receive before starting to buffer the connection.
    pub non_buffering_limit: u64,

    // Different limit on accepting invalid submissions on a single connection.
    pub invalid_flags_limit: u64,
    pub self_flags_limit: u64,
    pub expired_flags_limit: u64,
    pub duplicate_submission_limit: u64,

    // Max number of connections from a team.
    pub max_connection: u64,
}

impl Default for TcpConfig {
    fn default() -> Self {
        // Default values for TcpConfig.
        // TODO: Update values which make sense
        Self {
            non_buffering_limit: 200,
            invalid_flags_limit: 1_000,
            self_flags_limit: 1_000,
            expired_flags_limit: 1_000,
            duplicate_submission_limit: 1_000,
            max_connection: 50,
        }
    }
}

use derive_more::{Deref, DerefMut};
// Metrics which should be collected.
#[derive(Debug, Clone, Default, Deref, DerefMut)]
pub struct Metrics(HashMap<TeamId, FlagSubmissionMetrics>);

#[derive(Default, Debug, Clone, Deserialize, Serialize)]
pub struct FlagSubmissionMetrics {
    tcp_connections: u64,
    accepted: u64,
    invalid_flag: u64,
    duplicate_submission: u64,
    flag_expired: u64,
    self_flag: u64,
}

impl FlagSubmissionMetrics {
    pub fn get_tcp_connections(&self) -> u64 {
        self.tcp_connections
    }

    pub fn get_accepted(&self) -> u64 {
        self.accepted
    }

    pub fn get_invalid_flag(&self) -> u64 {
        self.invalid_flag
    }

    pub fn get_duplicate_submission(&self) -> u64 {
        self.duplicate_submission
    }

    pub fn get_flag_expired(&self) -> u64 {
        self.flag_expired
    }

    pub fn get_self_flag(&self) -> u64 {
        self.self_flag
    }
}

impl FlagSubmissionMetrics {
    fn update(&mut self, status: &FlagStatus) {
        match status {
            FlagStatus::Accepted => self.accepted += 1,
            FlagStatus::InvalidFlag => self.invalid_flag += 1,
            FlagStatus::DuplicateSubmission => self.duplicate_submission += 1,
            FlagStatus::FlagExpired => self.flag_expired += 1,
            FlagStatus::SelfFlag => self.self_flag += 1,
        }
    }

    fn total(&self) -> u64 {
        self.accepted
            + self.invalid_flag
            + self.duplicate_submission
            + self.flag_expired
            + self.self_flag
    }

    fn append(&mut self, metrics: &FlagSubmissionMetrics) {
        self.accepted += metrics.accepted;
        self.invalid_flag += metrics.invalid_flag;
        self.duplicate_submission += metrics.duplicate_submission;
        self.flag_expired += metrics.flag_expired;
        self.self_flag += metrics.self_flag;
    }
}

pub async fn get_flags_gained(
    model: &Model,
    team_id: TeamId,
    service_id: ServiceId,
) -> Vec<u64> {
    let gained = model.h_flag_gained.read().await.values(team_id, service_id);
    gained
}

pub async fn get_flags_lost(
    model: &Model,
    team_id: TeamId,
    service_id: ServiceId,
) -> Vec<u64> {
    model.h_flag_lost.read().await.values(team_id, service_id)
}
