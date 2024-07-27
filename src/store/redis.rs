use async_trait::async_trait;
use std::{fmt::Debug, sync::Arc};

use fred::clients::RedisClient;
use fred::types::{RedisKey, RedisMap};
use fred::{
    error::RedisError,
    interfaces::{HashesInterface, KeysInterface},
};
use serde::{de::DeserializeOwned, Serialize};

use crate::store::{Error, SessionStore};
use crate::Id;

#[derive(thiserror::Error, Debug)]
pub enum RedisStoreError {
    #[error(transparent)]
    Redis(#[from] RedisError),

    #[error(transparent)]
    Decode(#[from] rmp_serde::decode::Error),

    #[error(transparent)]
    Encode(#[from] rmp_serde::encode::Error),
}

impl From<RedisStoreError> for Error {
    fn from(err: RedisStoreError) -> Self {
        match err {
            RedisStoreError::Redis(inner) => Error::Backend(inner.to_string()),
            RedisStoreError::Decode(inner) => Error::Decode(inner.to_string()),
            RedisStoreError::Encode(inner) => Error::Encode(inner.to_string()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RedisStore<C: HashesInterface + KeysInterface + Clone + Send + Sync = RedisClient> {
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

impl From<Id> for RedisKey {
    fn from(value: Id) -> Self {
        value.as_ref().to_owned().into()
    }
}

#[async_trait]
impl<C> SessionStore for RedisStore<C>
where
    C: HashesInterface + KeysInterface + Clone + Send + Sync + 'static,
{
    async fn delete(&self, session_id: Id) -> Result<i32, Error> {
        let no_of_deleted_keys: i32 = self
            .client
            .del(session_id)
            .await
            .map_err(RedisStoreError::Redis)?;

        Ok(no_of_deleted_keys)
    }

    async fn expire(&self, session_id: Id, expire: i64) -> Result<bool, Error> {
        Ok(self
            .client
            .expire(session_id, expire)
            .await
            .map_err(RedisStoreError::Redis)?)
    }

    async fn get<T>(&self, session_id: Id, key: &str) -> Result<Option<T>, Error>
    where
        T: Clone + Send + Sync + DeserializeOwned,
    {
        let data = self
            .client
            .hget::<Option<Vec<u8>>, _, _>(session_id, key)
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

    async fn get_all<T>(&self, session_id: Id) -> Result<Option<T>, Error>
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
                rmp_serde::from_slice(&data).map_err(RedisStoreError::Decode)?,
            ))
        } else {
            Ok(None)
        }
    }

    async fn insert<T>(
        &self,
        session_id: Id,
        key: &str,
        value: &T,
        expire: i64,
    ) -> Result<bool, Error>
    where
        T: Send + Sync + Serialize,
    {
        let inserted: bool = self
            .client
            .hsetnx(
                session_id,
                key,
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

    async fn insert_many<T>(
        &self,
        session_id: Id,
        pairs: Vec<(&str, &T)>,
        expire: i64,
    ) -> Result<(), Error>
    where
        T: Send + Sync + Serialize,
    {
        let mut map = RedisMap::new();

        for (key, value) in pairs {
            map.insert(
                key.into(),
                rmp_serde::to_vec(value)
                    .map_err(RedisStoreError::Encode)?
                    .as_slice()
                    .into(),
            );
        }

        let inserted: bool = self
            .client
            .hset(session_id, map)
            .await
            .map_err(RedisStoreError::Redis)?;

        if inserted {
            // Set expiry on the root hash key
            self.expire(session_id, expire).await?;
        }

        Ok(())
    }

    async fn remove(&self, session_id: Id, key: &str) -> Result<bool, Error> {
        let removed: bool = self
            .client
            .hdel(session_id, key)
            .await
            .map_err(RedisStoreError::Redis)?;

        Ok(removed)
    }

    async fn update<T>(
        &self,
        session_id: Id,
        key: &str,
        value: &T,
        _expire: i64,
    ) -> Result<bool, Error>
    where
        T: Send + Sync + Serialize,
    {
        let mut map = RedisMap::new();
        map.insert(
            key.into(),
            rmp_serde::to_vec(value)
                .map_err(RedisStoreError::Encode)?
                .as_slice()
                .into(),
        );

        let updated: bool = self
            .client
            .hset(session_id, map)
            .await
            .map_err(RedisStoreError::Redis)?;

        Ok(updated)
    }

    async fn update_key(
        &self,
        old_session_id: Id,
        new_session_id: Id,
        expire: i64,
    ) -> Result<bool, Error> {
        let updated = self
            .client
            .renamenx(old_session_id, new_session_id)
            .await
            .map_err(RedisStoreError::Redis)?;

        if updated {
            // Set expiry on the root hash key
            self.expire(new_session_id, expire).await?;
        };

        Ok(updated)
    }
}
