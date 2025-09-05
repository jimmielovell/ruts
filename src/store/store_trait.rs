use crate::Id;
use dashmap::DashMap;
use serde::{de::DeserializeOwned, Serialize};
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
pub struct SessionMap(DashMap<String, Vec<u8>>);

impl SessionMap {
    pub(crate) fn new(map: DashMap<String, Vec<u8>>) -> Self {
        Self(map)
    }

    /// Deserializes a specific field from the session data into a requested type.
    ///
    /// Returns `Ok(None)` if the field does not exist, `Err` if deserialization fails,
    /// and `Ok(Some(value))` on success.
    pub fn get<T: DeserializeOwned>(&self, field: &str) -> Result<Option<T>, Error> {
        match self.0.get(field) {
            Some(bytes) => deserialize_value(&bytes).map(Some),
            None => Ok(None),
        }
    }

    #[cfg(feature = "layered-store")]
    pub(crate) fn iter(&self) -> dashmap::iter::Iter<'_, String, Vec<u8>> {
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
        T: Clone + Send + Sync + DeserializeOwned;

    /// Gets all the `field`-`value` pairs stored at `session_id`
    fn get_all(
        &self,
        session_id: &Id,
    ) -> impl Future<Output = Result<Option<SessionMap>, Error>> + Send;

    /// Sets a `field` stored at `session_id` to its provided `value`,
    /// only if the `field` does not exist.
    ///
    /// Returns `true` if the `field` was inserted, otherwise, `false` if the `field` already exists.
    fn insert<T>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        key_seconds: i64,
        field_seconds: Option<i64>,
    ) -> impl Future<Output = Result<bool, Error>> + Send
    where
        T: Send + Sync + Serialize + 'static;

    /// Updates a `field` stored at `session_id` to the new `value`.
    ///
    /// If the `field` does not exist, it is set to the corresponding `value`.
    ///
    /// Returns `true` if the `field` was updated, `false` if the `value` has not changed.
    fn update<T>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        key_seconds: i64,
        field_seconds: Option<i64>,
    ) -> impl Future<Output = Result<bool, Error>> + Send
    where
        T: Send + Sync + Serialize + 'static;

    /// Sets a `field` stored at `session_id` to its provided `value` and renames
    /// the session ID from `old_session_id` to `new_session_id`,
    /// only if the `field` does not exist.
    ///
    /// Returns `true` if the `field` was inserted, otherwise, `false` if the `field` already exists.
    fn insert_with_rename<T>(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        field: &str,
        value: &T,
        key_seconds: i64,
        field_seconds: Option<i64>,
    ) -> impl Future<Output = Result<bool, Error>> + Send
    where
        T: Send + Sync + Serialize + 'static;

    /// Updates a `field` stored at `session_id` to the new `value` and renames
    /// the session ID from `old_session_id` to `new_session_id`.
    ///
    /// If the `field` does not exist, it is set to the corresponding `value`.
    ///
    /// Returns `true` if the `field` was updated, `false` if the `value` has not changed.
    fn update_with_rename<T>(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        field: &str,
        value: &T,
        key_seconds: i64,
        field_seconds: Option<i64>,
    ) -> impl Future<Output = Result<bool, Error>> + Send
    where
        T: Send + Sync + Serialize + 'static;

    /// Renames the `old_session_id` to `new_session_id` if the `old_session_id` exists.
    ///
    /// Returns an error when `old_session_id` does not exist.
    fn rename_session_id(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        seconds: i64,
    ) -> impl Future<Output = Result<bool, Error>> + Send;

    /// Remove the `field` along with its `value` stored at `session_id`.
    ///
    /// Returns `1` if there are remaining keys or `0` otherwise.
    fn remove(
        &self,
        session_id: &Id,
        field: &str,
    ) -> impl Future<Output = Result<i8, Error>> + Send;

    /// Deletes all `field`s along with its `value`s stored in the `session_id`.
    fn delete(&self, session_id: &Id) -> impl Future<Output = Result<bool, Error>> + Send;

    /// Set a timeout on the `session_id`. After the timeout has expired,
    /// the `session_id` will be automatically deleted.
    ///
    /// A value of `-1` or `0` immediately expires the `session_id`.
    fn expire(
        &self,
        session_id: &Id,
        seconds: i64,
    ) -> impl Future<Output = Result<bool, Error>> + Send;
}

pub trait LayeredHotStore: Clone + Send + Sync + 'static {
    fn update_many(
        &self,
        session_id: &Id,
        pairs: &[(String, Vec<u8>, Option<i64>)],
    ) -> impl Future<Output = Result<bool, Error>> + Send;
}
