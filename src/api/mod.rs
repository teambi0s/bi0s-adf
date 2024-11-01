mod handlers;
mod models;
mod routes;

use anyhow::Context;
use axum::Router;
use handlers::admin_handlers::admin_routes;
use models::cache::Cache;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::net::TcpListener;
use tower_http::cors::{Any, CorsLayer};

use crate::ADMIN_API_PORT;
use crate::{
    game::{self, Comm},
    service::ServiceRegistry,
    teams::TeamRegistry,
    API_PORT,
};
use routes::game_routes;

pub use crate::api::handlers::admin_handlers::ApiCommand;

pub async fn game_api(
    teams: TeamRegistry,
    services: ServiceRegistry,
    comm: Comm,
    model: Arc<game::Model>,
) -> anyhow::Result<()> {
    let cache = Arc::new(Cache::new(&teams, &services));
    tokio::spawn(models::cache::update_cache(
        cache.clone(),
        comm.clone(),
        model.clone(),
    ));

    let app_state = handlers::AppState::new(cache, comm.clone(), model);

    let cors = CorsLayer::new().allow_origin(Any);
    let game_app = Router::new()
        .merge(game_routes())
        .layer(cors.clone())
        .with_state(app_state.clone());

    let admin_app = Router::new()
        .merge(admin_routes())
        .with_state(app_state.clone());

    // Uncomment the following line to use a different method for creating the game address
    // let game_addr = ([0, 0, 0, 0], API_PORT).into();
    let game_addr =
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), API_PORT);
    let game_listener = TcpListener::bind(game_addr)
        .await
        .context("Starting Game API Listener")?;

    let admin_addr = SocketAddr::new(
        IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
        ADMIN_API_PORT,
    ); // Add this line to specify the admin address
    let admin_listener = TcpListener::bind(admin_addr)
        .await
        .context("Starting Admin API Listener")?; // Add this line to bind the admin listener

    tracing::info!("Starting Game API Server @ {}", game_addr);
    tracing::info!("Starting Admin API Server @ {}", admin_addr); // Add this line to log the admin server start

    tokio::try_join!(
        axum::serve(game_listener, game_app),
        axum::serve(admin_listener, admin_app)
    )?;

    Ok(())
}
