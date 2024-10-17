use async_trait::async_trait;
use serde::{de::DeserializeOwned, Serialize};
use std::collections::HashMap;
use std::fmt::Debug;

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

#[async_trait]
pub trait SessionStore: Clone + Send + Sync + 'static {
    /// Gets the `value` for a `field` stored at `session_id`
    async fn get<T>(&self, session_id: &Id, field: &str) -> Result<Option<T>, Error>
    where
        T: Clone + Send + Sync + DeserializeOwned;

    /// Gets all the `field`-`value` pairs stored at `session_id`
    async fn get_all<T>(&self, session_id: &Id) -> Result<Option<HashMap<String, T>>, Error>
    where
        T: Clone + Send + Sync + DeserializeOwned;

    /// Sets a `field` stored at `session_id` to its provided `value`,
    /// only if the `field` does not exist.
    ///
    /// Returns `true` if the `field` was inserted, otherwise, `false` if the `field` already exists.
    async fn insert<T>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        seconds: i64,
    ) -> Result<bool, Error>
    where
        T: Send + Sync + Serialize;

    /// Updates a `field` stored at `session_id` to the new `value`.
    ///
    /// If the `field` does not exist, it is set to the corresponding `value`.
    ///
    /// Returns `true` if the `field` was updated, `false` if the `value` has not changed.
    async fn update<T>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        seconds: i64,
    ) -> Result<bool, Error>
    where
        T: Send + Sync + Serialize;

    /// Renames the `old_session_id` to `new_session_id` if the `old_session_id` exists.
    ///
    /// Returns an error when `old_session_id` does not exist.
    async fn rename_session_id(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        seconds: i64,
    ) -> Result<bool, Error>;

    /// Remove the `field` along with its `value` stored at `session_id`.
    ///
    /// Returns `1` if there are remaining keys or `0` otherwise.
    async fn remove(&self, session_id: &Id, field: &str) -> Result<i8, Error>;

    /// Deletes all `field`s along with its `value`s stored in the `session_id`.
    async fn delete(&self, session_id: &Id) -> Result<bool, Error>;

    /// Set a timeout on the `session_id`. After the timeout has expired,
    /// the `session_id` will be automatically deleted.
    ///
    /// A value of `-1` or `0` immediately expires the `session_id`.
    async fn expire(&self, session_id: &Id, seconds: i64) -> Result<bool, Error>;
}
