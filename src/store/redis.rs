use std::{fmt::Debug, sync::Arc};
use async_trait::async_trait;

use fred::types::{RedisKey, RedisMap};
use fred::{
    error::RedisError,
    interfaces::{HashesInterface, KeysInterface},
};
use serde::{de::DeserializeOwned, Serialize};

use crate::{Id, store};
use crate::store::SessionStore;

#[derive(thiserror::Error, Debug)]
pub enum RedisStoreError {
    #[error(transparent)]
    Redis(#[from] RedisError),

    #[error(transparent)]
    Decode(#[from] rmp_serde::decode::Error),

    #[error(transparent)]
    Encode(#[from] rmp_serde::encode::Error),
}

impl From<RedisStoreError> for store::Error {
    fn from(err: RedisStoreError) -> Self {
        match err {
            RedisStoreError::Redis(inner) => store::Error::Backend(inner.to_string()),
            RedisStoreError::Decode(inner) => store::Error::Decode(inner.to_string()),
            RedisStoreError::Encode(inner) => store::Error::Encode(inner.to_string()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RedisStore<C: HashesInterface + KeysInterface + Clone + Send + Sync> {
    client: Arc<C>,
}

impl<C> RedisStore<C>
where
    C: HashesInterface + KeysInterface + Clone + Send + Sync,
{
    pub fn new(client: Arc<C>) -> Self {
        Self { client }
    }
}

impl Into<RedisKey> for Id {
    fn into(self) -> RedisKey {
        self.as_ref().to_string().into()
    }
}

#[async_trait]
impl<C> SessionStore for RedisStore<C>
where
    C: HashesInterface + KeysInterface + Clone + Send + Sync + 'static,
{
    /// Delete the entire session from the store.
    ///
    /// - This method can fail if the store is unable to delete the session.
    async fn delete(&self, session_id: Id) -> Result<bool, store::Error> {
        let keys = self
            .client
            .hkeys::<Vec<String>, _>(session_id)
            .await
            .map_err(RedisStoreError::Redis)?;
        let deleted: bool = self
            .client
            .hdel(session_id, keys)
            .await
            .map_err(RedisStoreError::Redis)?;

        Ok(deleted)
    }

    async fn expire(&self, session_id: Id, expire: i64) -> Result<bool, store::Error> {
        Ok(self
            .client
            .expire(session_id, expire)
            .await
            .map_err(RedisStoreError::Redis)?
        )
    }

    /// Get a value from the store for a session.
    ///
    /// - This method can fail if the store is unable to get the value.
    async fn get<T>(&self, session_id: Id, field: &str) -> Result<Option<T>, store::Error>
    where
        T: Clone + Send + Sync + DeserializeOwned,
    {
        let data = self
            .client
            .hget::<Option<Vec<u8>>, _, _>(session_id, field)
            .await
            .map_err(RedisStoreError::Redis)?;

        if let Some(data) = data {
            Ok(Some(
                rmp_serde::from_slice(&data).map_err(RedisStoreError::Decode)?
            ))
        } else {
            Ok(None)
        }
    }

    /// Get all values from the store for a session.
    ///
    /// - This method can fail if the store is unable to get the values.
    async fn get_all<T>(&self, session_id: Id) -> Result<Option<T>, store::Error>
    where
        T: Clone + Send + Sync + DeserializeOwned,
    {
        let data = self
            .client
            .hgetall::<Option<Vec<u8>>, _>(session_id)
            .await
            .map_err(RedisStoreError::Redis)?;

        if let Some(data) = data {
            Ok(Some(
                rmp_serde::from_slice(&data).map_err(RedisStoreError::Decode)?
            ))
        } else {
            Ok(None)
        }
    }

    /// Set a value in the store for a session.
    async fn insert<T>(
        &self,
        session_id: Id,
        field: &str,
        value: &T,
        expire: i64,
    ) -> Result<bool, store::Error>
    where
        T: Send + Sync + Serialize,
    {

        let inserted: bool = self
            .client
            .hsetnx(
                session_id,
                field,
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

    /// Insert multiple values in to the session store.
    ///
    /// This is useful for when you want to insert multiple values at once.
    ///
    /// - This method can fail if the store is unable to insert the values.
    async fn insert_many<T>(&self, session_id: Id, pairs: &T, expire: i64) -> Result<(), store::Error>
        where
            T: Send + Sync + Serialize,
    {
        unimplemented!()
    }

    /// Removes the specified field from the session stored at id.
    ///
    /// - This method can fail if the store is unable to remove the value.
    async fn remove(&self, session_id: Id, field: &str) -> Result<bool, store::Error> {
        let removed: bool = self
            .client
            .hdel(session_id, field)
            .await
            .map_err(RedisStoreError::Redis)?;

        Ok(removed)
    }

    async fn update<T>(
        &self,
        session_id: Id,
        field: &str,
        value: &T,
    ) -> Result<bool, store::Error>
    where
        T: Send + Sync + Serialize,
    {
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
            .hset(session_id, &map)
            .await
            .map_err(RedisStoreError::Redis)?;

        Ok(updated)
    }

    async fn update_key(
        &self,
        old_session_id: Id,
        new_session_id: Id,
    ) -> Result<bool, store::Error> {
        Ok(self
            .client
            .renamenx(
                old_session_id,
                new_session_id,
            )
            .await
            .map_err(RedisStoreError::Redis)?
        )
    }
}
