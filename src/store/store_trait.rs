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
pub(crate) fn serialize_value<T: Serialize>(value: &T) -> Result<Vec<u8>, Error> {
    rmp_serde::to_vec(value).map_err(|e| Error::Encode(e.to_string()))
}

#[cfg(feature = "messagepack")]
pub(crate) fn deserialize_value<T: DeserializeOwned>(value: &[u8]) -> Result<T, Error> {
    rmp_serde::from_slice(value).map_err(|e| Error::Decode(e.to_string()))
}

#[cfg(feature = "bincode")]
pub(crate) fn serialize_value<T: Serialize>(value: &T) -> Result<Vec<u8>, Error> {
    let e = bincode::serde::encode_to_vec(value, bincode::config::standard())?;
    Ok(e)
}

#[cfg(feature = "bincode")]
pub(crate) fn deserialize_value<T: DeserializeOwned>(value: &[u8]) -> Result<T, Error> {
    let (d, _) = bincode::serde::decode_from_slice(value, bincode::config::standard())?;
    Ok(d)
}

#[derive(Debug, Clone)]
pub struct SessionMap(HashMap<String, Vec<u8>>);

impl SessionMap {
    pub(crate) fn new(map: HashMap<String, Vec<u8>>) -> Self {
        Self(map)
    }

    /// Deserializes a specific field from the session data into `T`.
    ///
    /// Returns `Ok(None)` if the field does not exist, `Err` if deserialization failed,
    /// and `Ok(Some(value))` on success.
    pub fn get<T: DeserializeOwned>(&self, field: &str) -> Result<Option<T>, Error> {
        match self.0.get(field) {
            Some(bytes) => deserialize_value(bytes).map(Some),
            None => Ok(None),
        }
    }

    /// Returns the number of elements in the map
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns true if the map contains no elements.
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

    /// Sets a `field` stored at `session_id` to the new `value`.
    ///
    /// Returns the new max_age of the session.
    fn set<T>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: Option<i64>,
        field_ttl_secs: Option<i64>,
        #[cfg(feature = "layered-store")] hot_cache_ttl_secs: Option<i64>,
        #[cfg(not(feature = "layered-store"))] _: Option<std::marker::PhantomData<()>>,
    ) -> impl Future<Output = Result<i64, Error>> + Send
    where
        T: Send + Sync + Serialize + 'static;

    /// Updates a `field` stored at `session_id` to the new `value` and renames
    /// the session ID from `old_session_id` to `new_session_id`.
    ///
    /// If the `field` does not exist, it is set to the corresponding `value`.
    ///
    /// Returns the new max_age of the session if the `field` was updated.
    #[allow(clippy::too_many_arguments)]
    fn set_and_rename<T>(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: Option<i64>,
        field_ttl_secs: Option<i64>,
        #[cfg(feature = "layered-store")] hot_cache_ttl_secs: Option<i64>,
        #[cfg(not(feature = "layered-store"))] _: Option<std::marker::PhantomData<()>>,
    ) -> impl Future<Output = Result<i64, Error>> + Send
    where
        T: Send + Sync + Serialize + 'static;

    /// Renames the `old_session_id` to `new_session_id` if the `old_session_id` exists.
    ///
    /// Returns an error when `old_session_id` does not exist.
    fn rename_session_id(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
    ) -> impl Future<Output = Result<bool, Error>> + Send;

    /// Remove the `field` along with its `value` stored at `session_id`.
    ///
    /// Returns the `TTL` of the entire session stored at `session_id`.
    /// - -1 if the session is persistent.
    /// - \> 0
    fn remove(
        &self,
        session_id: &Id,
        field: &str,
    ) -> impl Future<Output = Result<i64, Error>> + Send;

    /// Deletes all `field`s along with its `value`s stored in the `session_id`.
    fn delete(&self, session_id: &Id) -> impl Future<Output = Result<bool, Error>> + Send;

    /// Set a timeout on the `session_id`. After the timeout has expired,
    /// the `session_id` will be automatically deleted.
    ///
    /// A value of `-1` or `0` immediately expires the `session_id`.
    fn expire(
        &self,
        session_id: &Id,
        ttl_secs: i64,
    ) -> impl Future<Output = Result<bool, Error>> + Send;
}
