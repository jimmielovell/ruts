use crate::Id;
use serde::{Serialize, de::DeserializeOwned};
use std::collections::HashMap;
use std::fmt::Debug;
use std::future::Future;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Encoding failed with: {0}")]
    Encode(String),
    #[error("Decoding failed with: {0}")]
    Decode(String),
    #[error("{0}")]
    Backend(String),
}

#[cfg(feature = "redis-store")]
impl From<fred::error::Error> for Error {
    fn from(value: fred::error::Error) -> Self {
        Error::Backend(value.to_string())
    }
}

#[cfg(feature = "postgres-store")]
impl From<sqlx::Error> for Error {
    fn from(value: sqlx::Error) -> Self {
        Error::Backend(value.to_string())
    }
}

#[cfg(feature = "bincode")]
impl From<bincode::error::EncodeError> for Error {
    fn from(value: bincode::error::EncodeError) -> Self {
        Error::Backend(value.to_string())
    }
}

#[cfg(feature = "bincode")]
impl From<bincode::error::DecodeError> for Error {
    fn from(value: bincode::error::DecodeError) -> Self {
        Error::Backend(value.to_string())
    }
}

#[cfg(feature = "messagepack")]
pub fn serialize_value<T: Serialize>(value: &T) -> Result<Vec<u8>, Error> {
    rmp_serde::to_vec(value).map_err(|e| Error::Encode(e.to_string()))
}

#[cfg(feature = "messagepack")]
pub(crate) fn deserialize_value<T: DeserializeOwned>(value: &[u8]) -> Result<T, Error> {
    rmp_serde::from_slice(value).map_err(|e| Error::Decode(e.to_string()))
}

#[cfg(feature = "bincode")]
pub fn serialize_value<T: Serialize>(value: &T) -> Result<Vec<u8>, Error> {
    let e = bincode::serde::encode_to_vec(value, bincode::config::standard())?;
    Ok(e)
}

#[cfg(feature = "bincode")]
pub(crate) fn deserialize_value<T: DeserializeOwned>(value: &[u8]) -> Result<T, Error> {
    let (d, _) = bincode::serde::decode_from_slice(value, bincode::config::standard())?;
    Ok(d)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Ttl(i32);

impl Ttl {
    pub fn new(secs: i64) -> Result<Self, Error> {
        if secs <= 0 || secs > i32::MAX as i64 {
            return Err(Error::Backend(format!(
                "invalid ttl {secs}: must be 1..={}",
                i32::MAX
            )));
        }
        Ok(Ttl(secs as i32))
    }

    /// The validated TTL in seconds (always `1..=i32::MAX`).
    pub const fn get(self) -> i32 {
        self.0
    }
}

impl From<Ttl> for i32 {
    fn from(ttl: Ttl) -> i32 {
        ttl.0
    }
}

impl From<Ttl> for u64 {
    fn from(t: Ttl) -> u64 {
        t.0 as u64
    }
}

impl From<Ttl> for i64 {
    fn from(t: Ttl) -> i64 {
        t.0 as i64
    }
}

impl From<Ttl> for f64 {
    fn from(t: Ttl) -> f64 {
        t.0 as f64
    }
}

#[derive(Debug, Clone)]
pub struct SessionMap(HashMap<String, Vec<u8>>);

impl SessionMap {
    pub(crate) fn new(map: HashMap<String, Vec<u8>>) -> Self {
        Self(map)
    }

    pub fn get<T: DeserializeOwned>(&self, field: &str) -> Result<Option<T>, Error> {
        match self.0.get(field) {
            Some(bytes) => deserialize_value(bytes).map(Some),
            None => Ok(None),
        }
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[cfg(feature = "layered-store")]
    pub(crate) fn iter(&self) -> std::collections::hash_map::Iter<'_, String, Vec<u8>> {
        self.0.iter()
    }
}

pub trait SessionStore: Clone + Send + Sync + 'static {
    /// Gets the `value` for a `field` stored at `session_id`
    fn get<T>(
        &self,
        session_id: &Id,
        field: &str,
    ) -> impl Future<Output = Result<Option<T>, Error>> + Send
    where
        T: Send + Sync + DeserializeOwned;

    /// Gets all the `field`-`value` pairs stored at `session_id`
    fn get_all(
        &self,
        session_id: &Id,
    ) -> impl Future<Output = Result<Option<SessionMap>, Error>> + Send;

    /// Sets a `field` stored at `session_id` to the new `value` using a field-specific TTL.
    fn set<T>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        field_ttl: Ttl,
        #[cfg(feature = "layered-store")] hot_cache_ttl: Option<Ttl>,
        #[cfg(not(feature = "layered-store"))] _: Option<std::marker::PhantomData<()>>,
    ) -> impl Future<Output = Result<(), Error>> + Send
    where
        T: Send + Sync + Serialize;

    /// Updates a `field` stored at `session_id` to the new `value` and renames
    /// the session ID from `old_session_id` to `new_session_id`.
    #[allow(clippy::too_many_arguments)]
    fn set_and_rename<T>(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        field: &str,
        value: &T,
        field_ttl: Ttl,
        #[cfg(feature = "layered-store")] hot_cache_ttl: Option<Ttl>,
        #[cfg(not(feature = "layered-store"))] _: Option<std::marker::PhantomData<()>>,
    ) -> impl Future<Output = Result<(), Error>> + Send
    where
        T: Send + Sync + Serialize;

    /// Renames the `old_session_id` to `new_session_id` if the `old_session_id` exists.
    /// Acts as session-fixation protection.
    fn rename_session_id(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
    ) -> impl Future<Output = Result<bool, Error>> + Send;

    /// Remove the `field` along with its `value` stored at `session_id`.
    /// When the last field is removed, the session is functionally deleted.
    fn remove(
        &self,
        session_id: &Id,
        field: &str,
    ) -> impl Future<Output = Result<(), Error>> + Send;

    /// Deletes all `field`s along with their `value`s stored in the `session_id`.
    fn delete(&self, session_id: &Id) -> impl Future<Output = Result<bool, Error>> + Send;

    /// Extends the TTL of a specific `field` belonging to `session_id`.
    /// Returns `true` if the field existed and was active, `false` if it was missing or expired.
    /// Returns an error if `ttl` is 0.
    fn expire_field(
        &self,
        session_id: &Id,
        field: &str,
        ttl: Ttl,
    ) -> impl Future<Output = Result<bool, Error>> + Send;
}
