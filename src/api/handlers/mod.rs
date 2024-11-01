pub(crate) mod admin_handlers;
pub(crate) mod game_handlers;

use crate::game::{Comm, Model};
use std::sync::Arc;

use super::models::cache::Cache;

#[derive(Clone)]
pub struct AppState {
    pub cache: Arc<Cache>,
    pub comm: Comm,
    pub model: Arc<Model>,
}

impl AppState {
    pub fn new(cache: Arc<Cache>, comm: Comm, model: Arc<Model>) -> Self {
        Self { cache, comm, model }
    }
}
