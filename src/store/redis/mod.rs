mod lua;

use crate::store::redis::lua::{
    INSERT_SCRIPT, INSERT_SCRIPT_HASH, INSERT_WITH_RENAME_SCRIPT, INSERT_WITH_RENAME_SCRIPT_HASH,
    RENAME_SCRIPT, RENAME_SCRIPT_HASH, UPDATE_SCRIPT, UPDATE_SCRIPT_HASH,
    UPDATE_WITH_RENAME_SCRIPT, UPDATE_WITH_RENAME_SCRIPT_HASH,
};
use crate::store::{deserialize_value, serialize_value, Error, SessionStore};
use crate::Id;
use fred::clients::Pool;
use fred::interfaces::{HashesInterface, KeysInterface};
use fred::prelude::LuaInterface;
use serde::{de::DeserializeOwned, Serialize};
use std::{fmt::Debug, sync::Arc};
use tokio::sync::OnceCell;

/// A redis session store implementation.
///
/// It uses a Redis Hash to manage session data and supports
/// serialization/deserialization using [MessagePack](https://crates.io/crates/rmp-serde).
///
/// # Redis Version Requirements
///
/// This implementation uses Redis 7.4+ features for field-level expiration [HEXPIRE](https://redis.io/docs/latest/commands/hexpire/).
/// If you're using an earlier Redis version, field expiration will not work.
#[derive(Clone, Debug)]
pub struct RedisStore<
    C: HashesInterface + KeysInterface + LuaInterface + Clone + Send + Sync = Pool,
> {
    client: Arc<C>,
}

impl<C> RedisStore<C>
where
    C: HashesInterface + KeysInterface + LuaInterface + Clone + Send + Sync,
{
    pub fn new(client: Arc<C>) -> Self {
        Self { client }
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
        let value = self
            .client
            .hget::<Option<Vec<u8>>, _, _>(session_id, field.as_bytes())
            .await?;

        let deserialized = if let Some(value) = value {
            Some(deserialize_value::<T>(&value)?)
        } else {
            None
        };

        Ok(deserialized)
    }

    async fn get_all<T>(&self, session_id: &Id) -> Result<Option<T>, Error>
    where
        T: Clone + Send + Sync + DeserializeOwned,
    {
        let value = self
            .client
            .hgetall::<Option<Vec<u8>>, _>(session_id)
            .await?;

        let deserialized = if let Some(value) = value {
            Some(deserialize_value::<T>(&value)?)
        } else {
            None
        };

        Ok(deserialized)
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
        insert_update(
            Arc::clone(&self.client),
            vec![session_id],
            field,
            value,
            key_seconds,
            field_seconds,
            &INSERT_SCRIPT_HASH,
            INSERT_SCRIPT,
        )
        .await
    }

    async fn update<T>(
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
        insert_update(
            Arc::clone(&self.client),
            vec![session_id],
            field,
            value,
            key_seconds,
            field_seconds,
            &UPDATE_SCRIPT_HASH,
            UPDATE_SCRIPT,
        )
        .await
    }

    async fn insert_with_rename<T>(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        field: &str,
        value: &T,
        key_seconds: i64,
        field_seconds: Option<i64>,
    ) -> Result<bool, Error>
    where
        T: Send + Sync + Serialize,
    {
        insert_update(
            Arc::clone(&self.client),
            vec![old_session_id, new_session_id],
            field,
            value,
            key_seconds,
            field_seconds,
            &INSERT_WITH_RENAME_SCRIPT_HASH,
            INSERT_WITH_RENAME_SCRIPT,
        )
        .await
    }

    async fn update_with_rename<T>(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        field: &str,
        value: &T,
        key_seconds: i64,
        field_seconds: Option<i64>,
    ) -> Result<bool, Error>
    where
        T: Send + Sync + Serialize,
    {
        insert_update(
            Arc::clone(&self.client),
            vec![old_session_id, new_session_id],
            field,
            value,
            key_seconds,
            field_seconds,
            &UPDATE_WITH_RENAME_SCRIPT_HASH,
            UPDATE_WITH_RENAME_SCRIPT,
        )
        .await
    }

    async fn rename_session_id(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        seconds: i64,
    ) -> Result<bool, Error> {
        let hash = RENAME_SCRIPT_HASH
            .get_or_try_init(|| async {
                let hash = fred::util::sha1_hash(RENAME_SCRIPT);
                if !self.client.script_exists::<bool, _>(&hash).await? {
                    let _: () = self.client.script_load(RENAME_SCRIPT).await?;
                }
                Ok::<String, fred::error::Error>(hash)
            })
            .await?;

        let renamed: bool = self
            .client
            .evalsha(
                hash,
                vec![old_session_id, new_session_id],
                vec![seconds.to_string().as_bytes()],
            )
            .await?;

        Ok(renamed)
    }

    async fn remove(&self, session_id: &Id, field: &str) -> Result<i8, Error> {
        let removed: i8 = self.client.hdel(session_id, field.as_bytes()).await?;

        Ok(removed)
    }

    async fn delete(&self, session_id: &Id) -> Result<bool, Error> {
        Ok(self.client.del(session_id).await?)
    }

    async fn expire(&self, session_id: &Id, seconds: i64) -> Result<bool, Error> {
        Ok(self.client.expire(session_id, seconds, None).await?)
    }
}

async fn insert_update<C, T>(
    client: Arc<C>,
    session_ids: Vec<&Id>,
    field: &str,
    value: &T,
    key_seconds: i64,
    field_seconds: Option<i64>,
    once_cell: &OnceCell<String>,
    script: &str,
) -> Result<bool, Error>
where
    C: LuaInterface + Clone + Send + Sync + 'static,
    T: Send + Sync + Serialize,
{
    let hash = once_cell
        .get_or_try_init(|| async {
            let hash = fred::util::sha1_hash(script);
            if !client.script_exists::<bool, _>(&hash).await? {
                let _: () = client.script_load(script).await?;
            }

            Ok::<String, fred::error::Error>(hash)
        })
        .await?;

    let serialized_value = serialize_value(value)?;
    let field_seconds_bytes = field_seconds
        .map(|s| s.to_string().as_bytes().to_vec())
        .unwrap_or_else(|| b"".to_vec());

    let done: bool = client
        .evalsha(
            hash,
            session_ids,
            vec![
                field.as_bytes(),
                &serialized_value,
                key_seconds.to_string().as_bytes(),
                &field_seconds_bytes,
            ],
        )
        .await?;

    Ok(done)
}
