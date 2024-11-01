use crate::api::handlers::AppState;
use crate::api::models::cache::{
    get_flag_identifier, get_service_state, get_service_state_history,
};
use crate::db::journal::{self};
use crate::flagbot;
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use chrono::{DateTime, Utc};
use serde::Serialize;

pub async fn g_history_score(
    Path(team_id): Path<i64>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, StatusCode> {
    if let Some(team_id) =
        state.model.teams.ids().find(|&team| *team == team_id)
    {
        #[derive(Serialize)]
        struct Res {
            service_id: String,
            score: Vec<f64>,
        }
        let mut result = Vec::new();
        for service_id in state.model.services.ids() {
            let model = state.model.scorebot_model();
            let h_score = model.h_score.read().await;
            let h_score = h_score.values(team_id, service_id);
            let service_id = (*service_id).to_string();
            result.push(Res {
                service_id,
                score: h_score,
            });
        }

        Ok(Json(result))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

pub async fn g_history_flag(
    Path(team_id): Path<i64>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, StatusCode> {
    if let Some(team_id) =
        state.model.teams.ids().find(|&team| *team == team_id)
    {
        #[derive(Serialize)]
        struct Res {
            service_id: String,
            flag_gained: Vec<u64>,
            flag_lost: Vec<u64>,
        }

        let mut result = Vec::new();

        for service_id in state.model.services.ids() {
            let model = state.model.flagbot_model();
            let t_gained =
                flagbot::get_flags_gained(&model, team_id, service_id).await;
            let t_lost =
                flagbot::get_flags_lost(&model, team_id, service_id).await;

            let service_id = (*service_id).to_string();
            result.push(Res {
                service_id,
                flag_gained: t_gained,
                flag_lost: t_lost,
            })
        }
        Ok(Json(result))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

pub async fn g_history_service_state(
    Path((team_id, service_id)): Path<(i64, i64)>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, StatusCode> {
    if let Some(team_id) =
        state.model.teams.ids().find(|&team| *team == team_id)
    {
        if let Some(service_id) = state
            .model
            .services
            .ids()
            .find(|&service| *service == service_id)
        {
            let service_state: String = get_service_state_history(
                team_id,
                service_id,
                state.model.clone(),
            )
            .await
            .unwrap_or_default();

            Ok((json_headers(), service_state))
        } else {
            Err(StatusCode::BAD_REQUEST)
        }
    } else {
        Err(StatusCode::BAD_REQUEST)
    }
}

pub async fn g_status(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, StatusCode> {
    #[derive(Serialize)]
    struct Status {
        name: String,
        flag_life: u64,
        tick_time: u64,
        current_round: i64,
        flag_port: u16,
        total_round: u64,
        round_started_at: DateTime<Utc>,
    }

    let config = state.model.config.clone();
    let current_round = journal::RoundJournal::get_current(&state.model.db)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let round_start =
        journal::RoundJournal::get_start_time(&state.model.db, current_round)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let status = Status {
        name: config.name().to_string(),
        flag_life: config.flag_life(),
        tick_time: config.tick_time(),
        current_round: current_round.round_number,
        flag_port: config.flag_port(),
        total_round: config.total_round(),
        round_started_at: round_start.clone(),
    };

    Ok(Json(status))
}

pub async fn g_service_state(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, StatusCode> {
    let result = get_service_state(state.model).unwrap_or_default();

    Ok((json_headers(), result))
}

pub async fn g_teams(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, StatusCode> {
    let teams = state.cache.teams.clone();
    Ok((json_headers(), teams))
}

pub async fn g_services(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, StatusCode> {
    let services = state.cache.services.clone();
    Ok((json_headers(), services))
}

pub async fn g_scoreboard(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, StatusCode> {
    let scoreboard = state.cache.scoreboard.read().await.clone();
    Ok((json_headers(), scoreboard))
}

pub async fn g_flag_identifier(
    Path((team_id, service_id)): Path<(i64, i64)>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, StatusCode> {
    if let Some(team_id) =
        state.model.teams.ids().find(|&team| *team == team_id)
    {
        if let Some(service_id) = state
            .model
            .services
            .ids()
            .find(|&service| *service == service_id)
        {
            let identifier =
                get_flag_identifier(team_id, service_id, state.model.clone())
                    .unwrap_or_default();
            return Ok((json_headers(), identifier));
        } else {
            Err(StatusCode::BAD_REQUEST)
        }
    } else {
        Err(StatusCode::BAD_REQUEST)
    }
}

pub fn json_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        "application/json".to_string().parse().unwrap(),
    );
    headers
}
