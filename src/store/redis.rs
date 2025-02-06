use crate::store::{Error, SessionStore};
use crate::Id;
use fred::clients::Pool;
use fred::interfaces::{HashesInterface, KeysInterface};
use fred::types::{Key};
use serde::{de::DeserializeOwned, Serialize};
use std::{fmt::Debug, sync::Arc};
use std::sync::{OnceLock};
use fred::prelude::LuaInterface;

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

static LAZY_INSERT_HASH: OnceLock<String> = OnceLock::new();
static LAZY_UPDATE_HASH: OnceLock<String> = OnceLock::new();

const INSERT_SCRIPT: &str = r#"
local key = KEYS[1]
local field = ARGV[1]
local value = ARGV[2]
local key_seconds = tonumber(ARGV[3])
local field_seconds = ARGV[4]

local inserted = redis.call('HSETNX', key, field, value)
if inserted == 1 then
    redis.call('EXPIRE', key, key_seconds)
    if field_seconds ~= '' then
        redis.call('HEXPIRE', key, tonumber(field_seconds), 'FIELDS', 1, field)
    end
end
return inserted
"#;

const UPDATE_SCRIPT: &str = r#"
local key = KEYS[1]
local field = ARGV[1]
local value = ARGV[2]
local key_seconds = tonumber(ARGV[3])
local field_seconds = ARGV[4]

local updated = redis.call('HSET', key, field, value)
if updated == 1 then
    redis.call('EXPIRE', key, key_seconds)
    if field_seconds ~= '' then
        redis.call('HEXPIRE', key, tonumber(field_seconds), 'FIELDS', 1, field)
    end
end
return updated
"#;

/// Redis session store implementation.
///
/// # Redis Version Requirements
///
/// This implementation uses Redis 7.4+ features for field-level expiration (HEXPIRE).
/// If you're using an earlier Redis version, field expiration will not work.
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
    C: HashesInterface + KeysInterface + LuaInterface + Clone + Send + Sync + 'static,
{
    async fn get<T>(&self, session_id: &Id, field: &str) -> Result<Option<T>, Error>
    where
        T: Clone + Send + Sync + DeserializeOwned,
    {
        let data = self
            .client
            .hget::<Option<Vec<u8>>, _, _>(session_id, field.as_bytes())
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
        key_seconds: i64,
        field_seconds: Option<i64>,
    ) -> Result<bool, Error>
    where
        T: Send + Sync + Serialize,
    {
        if LAZY_INSERT_HASH.get().is_none() {
            let hash = fred::util::sha1_hash(INSERT_SCRIPT);
            if !self.client.script_exists::<bool, _>(&hash).await.map_err(RedisStoreError::Redis)? {
                let _: () = self.client.script_load(INSERT_SCRIPT).await.map_err(RedisStoreError::Redis)?;
            }

            LAZY_INSERT_HASH.set(hash).unwrap();
        };

        let serialized_value = self.serialize_value(value)?;
        let field_seconds_bytes = field_seconds
            .map(|s| s.to_string().as_bytes().to_vec())
            .unwrap_or_else(|| b"".to_vec());

        let inserted: bool = self.client.evalsha(LAZY_INSERT_HASH.get().unwrap(), vec![session_id], vec![
            field.as_bytes(),
            &serialized_value,
            key_seconds.to_string().as_bytes(),
            &field_seconds_bytes,
        ]).await.map_err(RedisStoreError::Redis)?;

        Ok(inserted)
    }

    async fn update<T>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        key_seconds: i64,
        field_seconds: Option<i64>
    ) -> Result<bool, Error>
    where
        T: Send + Sync + Serialize,
    {
        if LAZY_UPDATE_HASH.get().is_none() {
            let hash = fred::util::sha1_hash(UPDATE_SCRIPT);
            if !self.client.script_exists::<bool, _>(&hash).await.map_err(RedisStoreError::Redis)? {
                let _: () = self.client.script_load(UPDATE_SCRIPT).await.map_err(RedisStoreError::Redis)?;
            }

            LAZY_UPDATE_HASH.set(hash).unwrap();
        };

        let serialized_value = self.serialize_value(value)?;
        let field_seconds_bytes = field_seconds
            .map(|s| s.to_string().as_bytes().to_vec())
            .unwrap_or_else(|| b"".to_vec());

        let updated: bool = self.client.evalsha(LAZY_UPDATE_HASH.get().unwrap(), vec![session_id], vec![
            field.as_bytes(),
            &serialized_value,
            key_seconds.to_string().as_bytes(),
            &field_seconds_bytes,
        ]).await.map_err(RedisStoreError::Redis)?;

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
            .hdel(session_id, field.as_bytes())
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
