use std::{
    collections::{HashMap, HashSet},
    hash::{Hash, Hasher},
};

use serde::{Deserialize, Serialize};

use crate::{
    db::journal::RoundId,
    db::tables::{FlagId, ServiceId, SlotId, TeamId},
    teams::{Attacker, Victim},
};

pub const SECRET_LEN: usize = 16;

pub type FlagRegistry = HashMap<FlagId, Flag>;
pub type FlagSubmissionSet = HashSet<(Attacker, Victim, FlagId)>;

#[derive(
    Debug, Clone, Serialize, Deserialize, Hash, PartialEq, Eq, Default,
)]
#[repr(C)]
/// Stores the secret string related to a flag.
pub struct FlagSecret([u8; SECRET_LEN]);

impl FlagSecret {
    pub fn inner(&self) -> &[u8; SECRET_LEN] {
        &self.0
    }

    // Encode the byte array to a hex string.
    pub fn encode(&self) -> String {
        hex::encode(&self.0)
    }

    // Decode a hex string to a FlagSecret.
    pub fn decode(hex_str: &str) -> Option<[u8; SECRET_LEN]> {
        if hex_str.len() != SECRET_LEN * 2 {
            return None;
        }
        let bytes = hex::decode(hex_str).ok()?;
        let mut arr = [0u8; SECRET_LEN];
        arr.copy_from_slice(&bytes);
        Some(arr)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[repr(C)]
pub struct Flag {
    round: RoundId,
    slot: SlotId,
    service: ServiceId,
    team: TeamId,
    secret: FlagSecret,
    token: Token,
    identifier: FlagIdentifier,
}

impl Flag {
    pub fn new(
        round: RoundId,
        slot: SlotId,
        service: ServiceId,
        team: TeamId,
    ) -> Self {
        use rand::Rng;
        let secret = FlagSecret(rand::thread_rng().gen::<[u8; SECRET_LEN]>());
        let token = Token::new(None);
        let identifier = FlagIdentifier::new(None);
        Flag {
            round,
            slot,
            team,
            service,
            secret,
            token,
            identifier,
        }
    }

    pub fn import(
        round: RoundId,
        slot: SlotId,
        team: TeamId,
        service: ServiceId,
        secret: [u8; SECRET_LEN],
        token: Token,
        identifier: FlagIdentifier,
    ) -> Self {
        let secret = FlagSecret(secret);
        Flag {
            round,
            slot,
            service,
            team,
            secret,
            token,
            identifier,
        }
    }

    pub fn team_id(&self) -> TeamId {
        self.team
    }

    pub fn slot(&self) -> SlotId {
        self.slot
    }

    pub fn round_id(&self) -> RoundId {
        self.round
    }

    pub fn service_id(&self) -> ServiceId {
        self.service
    }

    pub fn token(&self) -> &Token {
        &self.token
    }

    pub fn identifier(&self) -> &FlagIdentifier {
        &self.identifier
    }

    pub fn secret(&self) -> &FlagSecret {
        &self.secret
    }
}

impl Hash for Flag {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.round.hash(state);
        self.slot.hash(state);
        self.team.hash(state);
    }
}

/// This trait is used to encode the flag secret in different formats,
/// which can change according to the game.
pub trait FlagCodec {
    /// Encodes the flag object into a printable format.
    fn encode(secret: &FlagSecret) -> String;
    /// Retrieves the encoded secret from the printable flag.
    fn decode(encoded: &[u8]) -> Option<FlagSecret>;
}

/// Token is a secret key associated with a flag, which can be
/// used to retrieve the flag. This is of type Option since we only get
/// a token if the service is working fine and we were able to plant the
/// flag.
#[derive(Debug, Hash, Clone, Serialize, Deserialize)]
pub struct Token(Option<String>);

impl Token {
    pub fn new(inner: Option<String>) -> Self {
        Self(inner)
    }

    pub fn to_string(&self) -> String {
        if let Some(inner) = &self.0 {
            inner.clone()
        } else {
            String::new()
        }
    }

    pub fn is_none(&self) -> bool {
        self.0.is_none()
    }
}

/// FlagIdentifier is a unique string used to retrieve a specific flag for a specific round.
#[derive(Debug, Hash, Clone, Serialize, Deserialize)]
pub struct FlagIdentifier(Option<String>);

impl FlagIdentifier {
    pub fn new(inner: Option<String>) -> Self {
        Self(inner)
    }

    pub fn to_string(&self) -> String {
        if let Some(inner) = &self.0 {
            inner.clone()
        } else {
            String::new()
        }
    }

    pub fn is_none(&self) -> bool {
        self.0.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, Hash, PartialEq)]
pub struct FlagKey {
    round: RoundId,
    slot: SlotId,
    team: TeamId,
    service: ServiceId,
}

impl From<&Flag> for FlagKey {
    fn from(flag: &Flag) -> Self {
        Self {
            round: flag.round_id(),
            slot: flag.slot(),
            team: flag.team_id(),
            service: flag.service_id(),
        }
    }
}

impl FlagKey {
    pub fn new(
        round: RoundId,
        slot: SlotId,
        team: TeamId,
        service: ServiceId,
    ) -> Self {
        Self {
            round,
            slot,
            team,
            service,
        }
    }

    pub fn round_id(&self) -> RoundId {
        self.round
    }
    pub fn slot(&self) -> SlotId {
        self.slot
    }
    pub fn team_id(&self) -> TeamId {
        self.team
    }

    pub fn service_id(&self) -> ServiceId {
        self.service
    }
}

pub struct DefaultFlag;

impl FlagCodec for DefaultFlag {
    fn encode(secret: &FlagSecret) -> String {
        format!("flag{{{}}}", hex::encode(secret.0))
    }
    fn decode(encoded: &[u8]) -> Option<FlagSecret> {
        if encoded.len() < (6 + SECRET_LEN * 2) {
            return None;
        }
        let encoded = &encoded[5..];
        let secret = hex::decode(&encoded[..(encoded.len() - 1)]).ok()?;
        let mut arr: [u8; SECRET_LEN] = [0; SECRET_LEN];
        arr.copy_from_slice(&secret);
        Some(FlagSecret(arr))
    }
}
