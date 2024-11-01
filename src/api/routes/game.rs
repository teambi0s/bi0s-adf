use crate::api::handlers::game_handlers::*;
use crate::api::handlers::AppState;
use axum::{routing::get, Router};

pub fn game_routes() -> Router<AppState> {
    Router::new()
        .route("/teams", get(g_teams))
        .route("/services", get(g_services))
        .route("/scoreboard", get(g_scoreboard))
        .route("/status", get(g_status))
        .route("/service_state", get(g_service_state))
        .route("/history/flag/:team_id", get(g_history_flag))
        .route("/history/score/:team_id", get(g_history_score))
        .route(
            "/history/service_state/:team_id/:service_id",
            get(g_history_service_state),
        )
        .route("/flagids/:team_id/:service_id", get(g_flag_identifier))
}
