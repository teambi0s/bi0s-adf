use std::collections::HashMap;

pub mod db;

pub mod config;
pub mod flag;
pub mod service;
pub mod teams;

pub mod api;
pub mod checker;
pub mod flagbot;
pub mod game;
pub mod history;
pub mod scoring;

pub mod dump;

pub const ADMIN_API_PORT: u16 = 31337;
pub const API_PORT: u16 = 31338;
pub const FLAG_SUBMISSION_PORT: u16 = 8080;

// Default open file limit
pub const DEFAULT_NOFILE_LIMIT: u64 = 1024 * 100;

#[derive(Clone)]
pub struct Registry<K, V>(HashMap<K, V>);

impl<K, V> Registry<K, V>
where
    K: Eq + std::hash::Hash + Clone,
{
    pub fn new(inner: HashMap<K, V>) -> Self {
        Self(inner)
    }

    pub fn ids(&self) -> impl Iterator<Item = K> + '_ {
        self.0.keys().cloned()
    }

    pub fn values(&self) -> impl Iterator<Item = &V> + '_ {
        self.0.values()
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        self.0.get(key)
    }

    pub fn inner(&self) -> &HashMap<K, V> {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

#[macro_export]
macro_rules! time {
    ($val:expr) => {
        {
            let beg = std::time::Instant::now();
            match $val {
                tmp => {
                    let end = std::time::Instant::now();
                    let time = (end - beg);
                    println!("[{}:{}] `{}' took {:?}", std::file!(), std::line!(), std::stringify!($val), time);
                    tmp
                }
            }
        }
    };
    ($($val:expr),+ $(,)?) => {
        ($(time!($val)),+,)
    };
}
