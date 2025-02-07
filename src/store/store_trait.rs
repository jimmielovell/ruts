use serde::{de::DeserializeOwned, Serialize};
use std::fmt::Debug;
use std::future::Future;

use crate::Id;

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
    fn get_all<T>(&self, session_id: &Id) -> impl Future<Output = Result<Option<T>, Error>> + Send
    where
        T: Clone + Send + Sync + DeserializeOwned;

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
        T: Send + Sync + Serialize;

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
        T: Send + Sync + Serialize;

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
        T: Send + Sync + Serialize;

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
        T: Send + Sync + Serialize;

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
