use std::str::FromStr;

use rand::{distributions::Standard, thread_rng, Rng};

/// A parsed on-demand session id.
#[derive(Clone, Debug)]
pub struct Id(String);

impl Id {
    pub fn new() -> Self {
        let id: Vec<u8> = thread_rng().sample_iter(Standard).take(128).collect();
        let hash = blake3::hash(&id);
        Self(hash.to_string())
    }

    pub fn parse(id: &str) -> Option<Self> {
        if id.len() == 128 {
            Some(Self(id.to_owned()))
        } else {
            None
        }
    }
}

impl ToString for Id {
    fn to_string(&self) -> String {
        self.0.clone()
    }
}

impl FromStr for Id {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.to_owned()))
    }
}
