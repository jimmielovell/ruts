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
    ///
    /// - This method can fail if the store is unable to delete the session.
    async fn delete(&self, session_id: Id) -> Result<bool, Error>;

    async fn expire(&self, session_id: Id, expire: i64) -> Result<bool, Error>;

    /// Get a value from the store for a session.
    ///
    /// - This method can fail if the store is unable to get the value.
    async fn get<T>(&self, session_id: Id, field: &str) -> Result<Option<T>, Error>
    where
        T: Clone + Send + Sync + DeserializeOwned;

    /// Get all values from the store for a session.
    ///
    /// - This method can fail if the store is unable to get the values.
    async fn get_all<T>(&self, session_id: Id) -> Result<Option<T>, Error>
    where
        T: Clone + Send + Sync + DeserializeOwned;

    /// Set a value in the store for a session.
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
        pairs: &T,
        expire: i64,
    ) -> Result<(), Error>
        where
            T: Send + Sync + Serialize;

    async fn remove(&self, session_id: Id, field: &str) -> Result<bool, Error>;

    async fn update<T>(
        &self,
        session_id: Id,
        field: &str,
        value: &T,
    ) -> Result<bool, Error>
        where
            T: Send + Sync + Serialize;

    async fn update_key(
        &self,
        old_session_id: Id,
        session_id: Id,
    ) -> Result<bool, Error>;
}
