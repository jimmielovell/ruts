use fred::clients::Pool;
use fred::interfaces::{HashesInterface, KeysInterface};
use fred::types::{Key, Value};
use serde::{de::DeserializeOwned, Serialize};
use std::collections::HashMap;
use std::{fmt::Debug, sync::Arc};

use crate::store::{Error, SessionStore};
use crate::Id;

#[derive(thiserror::Error, Debug)]
pub enum RedisStoreError {
    #[error(transparent)]
    Redis(#[from] fred::error::Error),

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
pub struct RedisStore<C: HashesInterface + KeysInterface + Clone + Send + Sync = Pool> {
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

impl From<&Id> for Key {
    fn from(value: &Id) -> Self {
        value.to_string().into()
    }
}

impl<C> RedisStore<C>
where
    C: HashesInterface + KeysInterface + Clone + Send + Sync + 'static,
{
    fn serialize_value(&self, value: impl Serialize) -> Result<Vec<u8>, RedisStoreError> {
        let mut msgpack_data = Vec::new();
        let mut serializer = rmp_serde::Serializer::new(&mut msgpack_data)
            .with_bytes(rmp_serde::config::BytesMode::ForceAll);
        value
            .serialize(&mut serializer)
            .map_err(RedisStoreError::Encode)?;

        Ok(msgpack_data)
    }

    fn deserialize_value<T>(&self, value: Option<Vec<u8>>) -> Result<Option<T>, RedisStoreError>
    where
        T: Clone + Send + Sync + DeserializeOwned,
    {
        Ok(if let Some(data) = value {
            Some(rmp_serde::from_slice(&data).map_err(RedisStoreError::Decode)?)
        } else {
            None
        })
    }
}

impl<C> SessionStore for RedisStore<C>
where
    C: HashesInterface + KeysInterface + Clone + Send + Sync + 'static,
{
    async fn get<T>(&self, session_id: &Id, field: &str) -> Result<Option<T>, Error>
    where
        T: Clone + Send + Sync + DeserializeOwned,
    {
        let data = self
            .client
            .hget::<Option<Vec<u8>>, _, _>(session_id, field)
            .await
            .map_err(RedisStoreError::Redis)?;

        Ok(self.deserialize_value::<T>(data)?)
    }

    async fn get_all<T>(&self, session_id: &Id) -> Result<Option<T>, Error>
    where
        T: Clone + Send + Sync + DeserializeOwned,
    {
        let data = self
            .client
            .hgetall::<Option<Vec<u8>>, _>(session_id)
            .await
            .map_err(RedisStoreError::Redis)?;

        Ok(self.deserialize_value::<T>(data)?)
    }

    async fn insert<T>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        seconds: i64,
    ) -> Result<bool, Error>
    where
        T: Send + Sync + Serialize,
    {
        let inserted: bool = self
            .client
            .hsetnx(session_id, field, self.serialize_value(value)?.as_slice())
            .await
            .map_err(RedisStoreError::Redis)?;

        if inserted {
            self.expire(session_id, seconds).await?;
        }

        Ok(inserted)
    }

    async fn update<T>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        seconds: i64,
    ) -> Result<bool, Error>
    where
        T: Send + Sync + Serialize,
    {
        let map: HashMap<Key, Value> =
            HashMap::from([(field.into(), self.serialize_value(value)?.as_slice().into())]);

        let updated: bool = self
            .client
            .hset(session_id, map)
            .await
            .map_err(RedisStoreError::Redis)?;

        if updated {
            self.expire(session_id, seconds).await?;
        }

        Ok(updated)
    }

    async fn rename_session_id(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        seconds: i64,
    ) -> Result<bool, Error> {
        let renamed: bool = self
            .client
            .renamenx(old_session_id, new_session_id)
            .await
            .map_err(RedisStoreError::Redis)?;

        if renamed {
            self.expire(new_session_id, seconds).await?;
        }

        Ok(renamed)
    }

    async fn remove(&self, session_id: &Id, field: &str) -> Result<i8, Error> {
        let removed: i8 = self
            .client
            .hdel(session_id, field)
            .await
            .map_err(RedisStoreError::Redis)?;

        Ok(removed)
    }

    async fn delete(&self, session_id: &Id) -> Result<bool, Error> {
        Ok(self
            .client
            .del(session_id)
            .await
            .map_err(RedisStoreError::Redis)?)
    }

    async fn expire(&self, session_id: &Id, seconds: i64) -> Result<bool, Error> {
        Ok(self
            .client
            .expire(session_id, seconds, None)
            .await
            .map_err(RedisStoreError::Redis)?)
    }
}
