use crate::api::handlers::game_handlers::json_headers;
use crate::api::handlers::AppState;
use crate::db::tables::TeamId;
use crate::flagbot;
use crate::flagbot::FlagSubmissionMetrics;
use crate::game;
use axum::routing::{get, post};
use axum::Router;
use axum::{extract::State, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Debug, Serialize)]
pub enum ApiCommand {
    Start,
    Pause,
    SetTcpConfig(flagbot::TcpConfig),
    ThrottleSubmission(bool),
    PauseFlagSubmission,
    StartFlagSubmission,
}

pub async fn command(
    State(state): State<AppState>,
    Json(command): Json<ApiCommand>,
) -> Result<impl IntoResponse, axum::http::StatusCode> {
    tracing::info!("Got Command : {:?}", &command);

    match &command {
        ApiCommand::Start => {
            state.comm.gamebot(game::Msg::Start).await;
        }
        ApiCommand::Pause => {
            state.comm.gamebot(game::Msg::Pause).await;
        }
        ApiCommand::SetTcpConfig(tcp_config) => {
            state
                .comm
                .flagbot(flagbot::Msg::UpdateTcpConfig(tcp_config.clone()))
                .await;
            // Restart flagbot for the configuration to be effective.
            state.comm.flagbot(flagbot::Msg::Pause).await;
            // Only works with a sleep. Needs some time to stop the connection.
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            state.comm.flagbot(flagbot::Msg::Start).await;
        }
        ApiCommand::PauseFlagSubmission => {
            state.comm.flagbot(flagbot::Msg::Pause).await;
        }
        ApiCommand::ThrottleSubmission(bool) => {
            state
                .comm
                .flagbot(flagbot::Msg::ThrottleSubmission(*bool))
                .await;
        }
        ApiCommand::StartFlagSubmission => {
            state.comm.flagbot(flagbot::Msg::Start).await;
        }
    }
    Ok(Json("Done"))
}

pub async fn g_submissions(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, axum::http::StatusCode> {
    let flagbot = state.model.flagbot_model();
    let metrics = flagbot.metrics().await;

    #[derive(Serialize)]
    struct Res {
        team_id: TeamId,
        metrics: FlagSubmissionMetrics,
    }

    let mut result = Vec::with_capacity(state.model.teams.len());

    for team_id in state.model.teams.ids() {
        metrics.get(&team_id).map(|metrics| {
            let res = Res {
                team_id,
                metrics: metrics.clone(),
            };
            result.push(res);
        });
    }

    let result = serde_json::to_string_pretty(&result).unwrap_or_default();
    Ok((json_headers(), result))
}

pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route("/admin", post(command))
        .route("/metrics/submissions", get(g_submissions))
}
