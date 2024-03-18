use std::{fmt::Debug, sync::Arc};

use fred::types::RedisMap;
use fred::{
    clients::RedisClient,
    error::RedisError,
    interfaces::{HashesInterface, KeysInterface},
};
use serde::{de::DeserializeOwned, Serialize};

use crate::Id;

#[derive(Debug, thiserror::Error)]
pub enum RedisStoreError {
    #[error(transparent)]
    Redis(#[from] RedisError),

    #[error(transparent)]
    Decode(#[from] rmp_serde::decode::Error),

    #[error(transparent)]
    Encode(#[from] rmp_serde::encode::Error),
}

#[derive(Clone, Debug)]
pub struct RedisStore {
    client: Arc<RedisClient>,
}

impl RedisStore {
    pub fn new(client: Arc<RedisClient>) -> Self {
        Self { client }
    }
}

impl RedisStore {
    /// Get a value from the store for a session.
    ///
    /// - This method can fail if the store is unable to get the value.
    pub async fn get<T>(&self, session_id: Id, field: &str) -> Result<Option<T>, RedisStoreError>
        where
            T: Clone + Send + Sync + DeserializeOwned,
    {
        let data = self
            .client
            .hget::<Option<Vec<u8>>, _, _>(session_id.inner_to_string(), field.to_string())
            .await
            .map_err(RedisStoreError::Redis)?;

        if let Some(data) = data {
            Ok(Some(
                rmp_serde::from_slice(&data).map_err(RedisStoreError::Decode)?,
            ))
        } else {
            Ok(None)
        }
    }

    /// Get all values from the store for a session.
    ///
    /// - This method can fail if the store is unable to get the values.
    pub async fn get_all<T>(&self, session_id: Id) -> Result<Option<T>, RedisStoreError>
        where
            T: Clone + Send + Sync + DeserializeOwned,
    {
        let data = self
            .client
            .hgetall::<Option<Vec<u8>>, _>(session_id.to_string())
            .await
            .map_err(RedisStoreError::Redis)?;

        if let Some(data) = data {
            Ok(Some(
                rmp_serde::from_slice(&data).map_err(RedisStoreError::Decode)?,
            ))
        } else {
            Ok(None)
        }
    }

    /// Set a value in the store for a session.
    pub async fn insert<T>(
        &self,
        session_id: Id,
        field: &str,
        value: &T,
        expire: i64,
    ) -> Result<bool, RedisStoreError>
        where
            T: Send + Sync + Serialize,
    {
        let key = session_id.inner_to_string();

        let inserted: bool = self
            .client
            .hsetnx(
                key.clone(),
                field.to_string(),
                rmp_serde::to_vec(value)
                    .map_err(RedisStoreError::Encode)?
                    .as_slice(),
            )
            .await
            .map_err(RedisStoreError::Redis)?;

        if inserted {
            // Set expiry on the root hash key
            self.expire(session_id, expire).await?;
        }

        Ok(inserted)
    }

    pub async fn update<T>(
        &self,
        session_id: Id,
        field: &str,
        value: &T,
    ) -> Result<bool, RedisStoreError>
        where
            T: Send + Sync + Serialize,
    {
        let key = session_id.inner_to_string();
        let mut map = RedisMap::new();
        map.insert(
            field.into(),
            rmp_serde::to_vec(value)
                .map_err(RedisStoreError::Encode)?
                .as_slice()
                .into(),
        );

        let updated: bool = self
            .client
            .hset(key.clone(), map)
            .await
            .map_err(RedisStoreError::Redis)?;

        Ok(updated)
    }

    pub async fn expire(&self, session_id: Id, expire: i64) -> Result<bool, RedisStoreError> {
        Ok(self
            .client
            .expire(session_id.inner_to_string(), expire)
            .await?)
    }

    pub async fn update_key(&self, old_session_id: Id, session_id: Id) -> Result<bool, RedisStoreError> {
        Ok(self
            .client
            .renamenx(
                old_session_id.inner_to_string(),
                session_id.inner_to_string(),
            )
            .await?)
    }

    /// Insert multiple values in to the session store.
    ///
    /// This is useful for when you want to insert multiple values at once.
    ///
    /// - This method can fail if the store is unable to insert the values.
    // pub async fn minsert<T>(&self, session_id: Id, pairs: &T, expire: i64) -> Result<(), RedisStoreError>
    // where
    //     T: Send + Sync + Serialize,
    // {
    //     let key = session_id.inner_to_string();
    //     // Set expiry on the root hash key
    //     self.client.expire(key.clone(), expire).await?;
    //
    //     self.client
    //         .hmset(key, rmp_serde::to_vec(pairs).map_err(RedisStoreError::Encode)?.as_slice())
    //         .await
    //         .map_err(RedisStoreError::Redis)?;
    //     Ok(())
    // }

    /// Removes the specified field from the session stored at id.
    ///
    /// - This method can fail if the store is unable to remove the value.
    pub async fn remove(&self, session_id: Id, field: &str) -> Result<bool, RedisStoreError> {
        let removed: bool = self
            .client
            .hdel(session_id.inner_to_string(), field.to_string())
            .await
            .map_err(RedisStoreError::Redis)?;

        Ok(removed)
    }

    /// Delete the entire session from the store.
    ///
    /// - This method can fail if the store is unable to delete the session.
    pub async fn delete(&self, session_id: Id) -> Result<bool, RedisStoreError> {
        let keys = self
            .client
            .hkeys::<Vec<String>, _>(session_id.inner_to_string())
            .await
            .map_err(RedisStoreError::Redis)?;
        let deleted: bool = self
            .client
            .hdel(session_id.inner_to_string(), keys)
            .await
            .map_err(RedisStoreError::Redis)?;

        Ok(deleted)
    }
}
