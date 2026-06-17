use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::prelude::BASE64_URL_SAFE_NO_PAD;
use rand::Rng;
use rand::prelude::StdRng;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::fmt::Display;
use std::str::FromStr;
use std::{fmt, str};

thread_local! {
    static RNG: RefCell<StdRng> = RefCell::new(rand::make_rng());
}

#[derive(Copy, Clone, Eq, Hash, PartialEq, Deserialize, Serialize)]
pub struct Id([u8; 22]);

impl Default for Id {
    fn default() -> Self {
        let mut bytes = [0u8; 16];
        RNG.with(|rng| rng.borrow_mut().fill_bytes(&mut bytes));

        let mut encoded = [0; 22];
        let _ = BASE64_URL_SAFE_NO_PAD.encode_slice(bytes, &mut encoded);

        Self(encoded)
    }
}

impl Id {
    /// Returns the string slice of the encoded Id.
    #[inline]
    pub fn as_str(&self) -> &str {
        str::from_utf8(&self.0).expect("Encoded Id is valid UTF-8")
    }
}

impl Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Id {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() != 22 {
            return Err("Invalid ID length: must be exactly 22 characters");
        }

        let mut decoded_buffer = [0u8; 16];
        if URL_SAFE_NO_PAD
            .decode_slice(s.as_bytes(), &mut decoded_buffer)
            .is_err()
        {
            return Err("Invalid ID characters: must be URL-safe Base64");
        }

        let mut bytes = [0u8; 22];
        bytes.copy_from_slice(s.as_bytes());
        Ok(Self(bytes))
    }
}

#[cfg(feature = "redis-store")]
impl From<&Id> for fred::types::Key {
    fn from(value: &Id) -> Self {
        value.as_str().into()
    }
}
