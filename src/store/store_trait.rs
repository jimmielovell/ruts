use async_trait::async_trait;
use serde::{de::DeserializeOwned, Serialize};
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
    /// Delete the entire session from the store.
    /// Returns the number of deleted keys.
    async fn delete(&self, session_id: Id) -> Result<i32, Error>;

    /// Update the session expiry.
    /// A value of -1 or 0 immediately expires the session and is deleted from the store.
    /// Returns a boolean indicating if the operation is successful or not.
    async fn expire(&self, session_id: Id, expire: i64) -> Result<bool, Error>;

    /// Get a value from the store for a session.
    /// Returns a boolean indicating if the operation is successful or not.
    async fn get<T>(&self, session_id: Id, field: &str) -> Result<Option<T>, Error>
    where
        T: Clone + Send + Sync + DeserializeOwned;

    /// Get all values from the store for a session.
    async fn get_all<T>(&self, session_id: Id) -> Result<Option<T>, Error>
    where
        T: Clone + Send + Sync + DeserializeOwned;

    /// Set a value in the store for a session field.
    /// Returns a boolean indicating if the operation is successful or not.
    async fn insert<T>(
        &self,
        session_id: Id,
        field: &str,
        value: &T,
        expire: i64,
    ) -> Result<bool, Error>
    where
        T: Send + Sync + Serialize;

    async fn insert_many<T>(
        &self,
        session_id: Id,
        pairs: Vec<(&str, &T)>,
        expire: i64,
    ) -> Result<(), Error>
    where
        T: Send + Sync + Serialize;

    /// Remove the session field along with its value from the session.
    /// Returns 1 if there are remaining keys or 0 otherwise. A value of -1 means the operation
    /// was not successful.
    async fn remove(&self, session_id: Id, field: &str) -> Result<bool, Error>;

    /// Update a value in the store for a session field.
    /// Inserts the value if the session field does not exist.
    async fn update<T>(
        &self,
        session_id: Id,
        field: &str,
        value: &T,
        expire: i64,
    ) -> Result<bool, Error>
    where
        T: Send + Sync + Serialize;

    /// Updates the session id.
    async fn update_key(
        &self,
        old_session_id: Id,
        session_id: Id,
        expire: i64,
    ) -> Result<bool, Error>;
}
