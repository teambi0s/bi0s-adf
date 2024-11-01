use crate::{
    db::journal::RoundId,
    db::tables::{FlagId, ServiceId, SlotId, TeamId},
    flag::{Flag, FlagCodec, FlagIdentifier, Token},
    service::{self, Service, ServiceRegistry, SlotRegistry},
    teams::TeamRegistry,
};

use futures::future::join_all;

use tokio::{
    net::TcpStream,
    sync::{
        mpsc::{channel, Receiver, Sender},
        oneshot,
    },
    time::sleep,
};

use tonic::{transport::Channel, Status};

use anyhow::Result;
use tracing::{debug, error, info, warn};

use std::{collections::HashMap, time::Duration};

// Include checker gRPC auto-generated code.
tonic::include_proto!("checker");

/// Messages which can be sent to the service checker.
#[derive(Debug, Clone)]
pub enum RequestMsg {
    PlantFlag(FlagId, Flag),
    CheckFlag(Flag, Token),
    CheckService(TeamId, ServiceId, RoundId),
}

#[derive(Debug)]
pub enum ResponseMsg {
    Failed(Error),
    ServiceState(RoundId, ServiceId, TeamId, service::ServiceState),
    PlantedFlag(
        RoundId,
        SlotId,
        TeamId,
        service::ServiceState,
        FlagId,
        Token,
        FlagIdentifier,
    ),
}

pub type Response = oneshot::Sender<ResponseMsg>;
pub type Request = (Response, RequestMsg);

pub struct CheckerBot {
    service_map: ServiceRegistry,
    teams_map: TeamRegistry,
    slot_map: SlotRegistry,
    comm: (Sender<Request>, Receiver<Request>),
}

#[derive(Debug)]
pub enum Error {
    RPCError(RPCError),
}

#[derive(Debug)]
pub enum RPCError {
    RequestFailed(Status),
    Transport(tonic::transport::Error),
    InvalidResponse,
}

impl CheckerBot {
    pub fn new(
        service_map: ServiceRegistry,
        teams_map: TeamRegistry,
        slot_map: SlotRegistry,
    ) -> Self {
        let comm = channel(10);
        Self {
            service_map,
            teams_map,
            slot_map,
            comm,
        }
    }

    pub fn sender(&self) -> Sender<Request> {
        self.comm.0.clone()
    }

    pub async fn run<F: FlagCodec + 'static>(mut self) {
        info!("Starting CheckerBot");
        let mut map = HashMap::new();
        for service_id in self.service_map.ids() {
            let (sender, receiver): (Sender<Request>, Receiver<Request>) =
                channel(10);
            // For every service, we create a dedicated process which is responsible
            // for handling the service.
            let service = self.service_map.get(&service_id).unwrap().clone();
            let teams_map = self.teams_map.clone();
            tokio::spawn(async move {
                let res =
                    RequestMsg::handle::<F>(service, teams_map, receiver).await;

                if let Err(e) = res {
                    error!("Service Handler returned Error: {:?}", e);
                }
            });
            map.insert(service_id, sender);
        }

        // Proxy incoming messages to the appropriate process.
        while let Some(request) = self.comm.1.recv().await {
            let handle_send_result = |x| {
                if let Err(e) = x {
                    error!("Unable to proxy msg to checker process: {}", e);
                }
            };

            debug!("Got Request for CheckerBot: {:?}", &request);
            match &request.1 {
                RequestMsg::PlantFlag(_flag_id, flag) => {
                    let slot_id = flag.slot();
                    let sender = self
                        .slot_map
                        .get(&slot_id)
                        .and_then(|service_id| map.get(&service_id));

                    if let Some(sender) = sender {
                        let res = sender.send(request).await;
                        handle_send_result(res);
                    }
                }
                RequestMsg::CheckFlag(flag, _token) => {
                    let slot_id = flag.slot();
                    let sender = self
                        .slot_map
                        .get(&slot_id)
                        .and_then(|service_id| map.get(&service_id));

                    if let Some(sender) = sender {
                        let res = sender.send(request).await;
                        handle_send_result(res);
                    }
                }
                RequestMsg::CheckService(_team_id, service_id, _round_id) => {
                    if let Some(sender) = map.get(service_id) {
                        let res = sender.send(request).await;
                        handle_send_result(res);
                    }
                }
            }
        }
    }

    async fn is_checker_up(&self, service_id: ServiceId) -> (ServiceId, bool) {
        let service = self.service_map.get(&service_id).unwrap();
        for i in 1..=5 {
            let tcp = TcpStream::connect(format!(
                "{}:{}",
                service.checker_host(),
                service.checker_port()
            ))
            .await
            .map_err(|err| {
                warn!(
                    "Unable to connect to checker for <{}>: {}",
                    service.name(),
                    err
                );
            });
            if tcp.is_ok() {
                return (service_id, true);
            }

            sleep(Duration::from_secs(2 * i)).await;
        }
        return (service_id, false);
    }

    pub async fn is_checkers_up(&self) -> Vec<(ServiceId, bool)> {
        let mut futures = Vec::new();
        for service_id in self.service_map.ids() {
            futures.push(self.is_checker_up(service_id));
        }
        join_all(futures).await
    }
}

impl RequestMsg {
    async fn handle<F: FlagCodec>(
        service: Service,
        teams: TeamRegistry,
        mut receiver: Receiver<Request>,
    ) -> Result<(), Error> {
        debug!("Starting Service Process for: {}", service.name());

        // Connect to gRPC checker server
        let uri = String::from(service.checker());
        let channel =
            tonic::transport::channel::Endpoint::from_shared(uri.clone())
                .expect(&format!("Invalid checker URI specified: {} !!", uri));

        let channel = channel
            .timeout(Duration::from_secs(service.timeout() as u64))
            .concurrency_limit(service.concurrency_limit())
            .connect_lazy();

        let client = checker_client::CheckerClient::new(channel);

        while let Some(request) = receiver.recv().await {
            let (response, msg) = request;
            match msg {
                RequestMsg::PlantFlag(flag_id, flag) => {
                    if let Some(team) = teams.get(&flag.team_id()) {
                        let flag_request = PlantFlagRequest {
                            ip: team.ip().to_string(),
                            port: service.port() as u32,
                            flag: F::encode(flag.secret()),
                            slot: flag.slot().get_slot_number() as u32,
                        };
                        tokio::spawn(plant_flag(
                            response,
                            client.clone(),
                            flag,
                            flag_id,
                            flag_request,
                        ));
                    } else {
                        warn!("Got invalid TeamId while processing RequestMsg::PlantFlag");
                    }
                }
                RequestMsg::CheckFlag(flag, token) => {
                    if let Some(team) = teams.get(&flag.team_id()) {
                        let check_request = CheckFlagRequest {
                            ip: team.ip().to_string(),
                            port: service.port() as u32,
                            flag: F::encode(flag.secret()),
                            token: token.to_string(),
                            identifier: flag.identifier().to_string(),
                            slot: flag.slot().get_slot_number() as u32,
                        };

                        tokio::spawn(check_flag(
                            response,
                            client.clone(),
                            flag,
                            check_request,
                        ));
                    } else {
                        error!("Got invalid TeamId while processing RequestMsg::CheckFlag")
                    }
                }

                RequestMsg::CheckService(team_id, service_id, round_id) => {
                    if let Some(team) = teams.get(&team_id) {
                        let check_request = CheckServiceRequest {
                            ip: team.ip().to_string(),
                            port: service.port() as u32,
                        };
                        tokio::spawn(check_service(
                            response,
                            client.clone(),
                            check_request,
                            team_id,
                            service_id,
                            round_id,
                        ));
                    } else {
                        error!("Got invalid TeamId while processing RequestMsg::CheckService");
                    }
                }
            }
        }
        Ok(())
    }
}

// Contains a mapping between serviceId and its corresponding checker process
#[derive(Debug, Clone)]
pub struct CheckerMap(HashMap<ServiceId, Sender<Request>>);

impl CheckerMap {
    pub fn sender(&self, id: ServiceId) -> Sender<Request> {
        self.0.get(&id).unwrap().clone()
    }
}

fn respond_error(resp: Response, error: Error) {
    let response = ResponseMsg::Failed(error);
    let res = resp.send(response);
    checker_response_result(res);
}

async fn plant_flag(
    response: Response,
    mut client: checker_client::CheckerClient<Channel>,
    flag: Flag,
    flag_id: FlagId,
    flag_request: PlantFlagRequest,
) {
    let request = tonic::Request::new(flag_request);
    let result = match client.plant_flag(request).await {
        Ok(response) => response.into_inner(),
        Err(err) => {
            let error = Error::RPCError(RPCError::RequestFailed(err));
            respond_error(response, error);
            return;
        }
    };

    let token = Token::new(Some(result.token));
    let identifier = FlagIdentifier::new(Some(result.identifier));

    let state = if let Some(status) = result.status {
        service::ServiceState::from(status)
    } else {
        // Response did not contain the status field. Review checker server.
        // Make sure the status is being sent.
        let error = Error::RPCError(RPCError::InvalidResponse);
        respond_error(response, error);
        return;
    };

    let response_msg = ResponseMsg::PlantedFlag(
        flag.round_id(),
        flag.slot(),
        flag.team_id(),
        state,
        flag_id,
        token,
        identifier,
    );
    let res = response.send(response_msg);
    checker_response_result(res);
}

async fn check_flag(
    response: Response,
    mut client: checker_client::CheckerClient<Channel>,
    flag: Flag,
    check_request: CheckFlagRequest,
) {
    let request = tonic::Request::new(check_request);

    let state = match client.check_flag(request).await {
        Ok(response) => service::ServiceState::from(response.into_inner()),
        Err(err) => {
            let error = Error::RPCError(RPCError::RequestFailed(err));
            respond_error(response, error);
            return;
        }
    };
    let res = response.send(ResponseMsg::ServiceState(
        flag.round_id(),
        flag.service_id(),
        flag.team_id(),
        state,
    ));
    checker_response_result(res);
}

async fn check_service(
    response: Response,
    mut client: checker_client::CheckerClient<Channel>,
    service_request: CheckServiceRequest,
    team_id: TeamId,
    service_id: ServiceId,
    round_id: RoundId,
) {
    let request = tonic::Request::new(service_request);

    let state = match client.check_service(request).await {
        Ok(response) => service::ServiceState::from(response.into_inner()),
        Err(err) => {
            let error = Error::RPCError(RPCError::RequestFailed(err));
            respond_error(response, error);
            return;
        }
    };
    let res = response.send(ResponseMsg::ServiceState(
        round_id, service_id, team_id, state,
    ));
    checker_response_result(res);
}

fn checker_response_result(r: Result<(), ResponseMsg>) {
    if let Err(e) = r {
        error!("Unable to send response back: {:?}", e);
    }
}

impl From<ServiceStatus> for service::ServiceState {
    fn from(status: ServiceStatus) -> Self {
        let s = status.state;
        match s {
            s if s == ServiceState::Unknown as i32 => {
                service::ServiceState::Unknown
            }
            s if s == ServiceState::Up as i32 => service::ServiceState::Up,
            s if s == ServiceState::Down as i32 => service::ServiceState::Down,
            s if s == ServiceState::Mumble as i32 => {
                service::ServiceState::Mumble(status.reason)
            }
            s if s == ServiceState::Corrupt as i32 => {
                service::ServiceState::Corrupt(status.reason)
            }
            _ => service::ServiceState::Unknown,
        }
    }
}
