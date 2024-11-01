use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use std::fmt::Debug;

use crate::db::{
    journal::RoundId,
    tables::{ServiceId, TeamId},
};

type Key = (TeamId, ServiceId, RoundId);

pub struct History<V> {
    history: HashMap<Key, V>,
}

impl<V> History<V>
where
    V: Serialize + for<'a> Deserialize<'a> + Clone + Debug,
{
    pub fn new() -> Self {
        let history = HashMap::new();
        Self { history }
    }

    pub fn insert(
        &mut self,
        team_id: TeamId,
        service_id: ServiceId,
        round_id: RoundId,
        value: V,
    ) {
        self.history
            .entry((team_id, service_id, round_id))
            .and_modify(|x| *x = value.clone())
            .or_insert(value.clone());
    }

    pub fn get(
        &self,
        team_id: TeamId,
        service_id: ServiceId,
        round_id: RoundId,
    ) -> Option<&V> {
        self.history.get(&(team_id, service_id, round_id))
    }

    pub fn values(&self, team_id: TeamId, service_id: ServiceId) -> Vec<V> {
        let mut collection: Vec<(RoundId, V)> = self
            .history
            .iter()
            .filter_map(|((t, s, round_id), value)| {
                if team_id == *t && *s == service_id {
                    Some((*round_id, value.clone()))
                } else {
                    None
                }
            })
            .collect();

        collection.sort_by_key(|(k, _)| k.id);

        let result = collection.into_iter().map(|(_, v)| v).collect();
        result
    }
}
