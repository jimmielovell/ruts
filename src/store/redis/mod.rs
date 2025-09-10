mod lua;

use crate::Id;
use crate::store::redis::lua::{
    INSERT_SCRIPT, INSERT_SCRIPT_HASH, INSERT_WITH_RENAME_SCRIPT, INSERT_WITH_RENAME_SCRIPT_HASH,
    UPDATE_MANY_SCRIPT, UPDATE_MANY_SCRIPT_HASH, UPDATE_SCRIPT,
    UPDATE_SCRIPT_HASH, UPDATE_WITH_RENAME_SCRIPT, UPDATE_WITH_RENAME_SCRIPT_HASH,
};
use crate::store::{Error, SessionMap, SessionStore, deserialize_value, serialize_value};
use fred::clients::Pool;
use fred::interfaces::{HashesInterface, KeysInterface};
use fred::prelude::LuaInterface;
use fred::types::Value;
use serde::{Serialize, de::DeserializeOwned};
use std::collections::HashMap;
use std::{fmt::Debug, sync::Arc};
use tokio::sync::OnceCell;

/// A redis session store implementation.
///
/// It uses a Redis Hash to manage session data
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
        T: Send + Sync + DeserializeOwned,
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

    async fn get_all(&self, session_id: &Id) -> Result<Option<SessionMap>, Error> {
        let result = self
            .client
            .hgetall::<Option<HashMap<String, Vec<u8>>>, _>(session_id)
            .await?;

        if result.is_none() {
            return Ok(None);
        }

        let result = result.unwrap();
        let mut map = HashMap::with_capacity(result.len());
        result.into_iter().for_each(|(field, value)| {
            map.insert(field, value);
        });

        Ok(Some(SessionMap::new(map)))
    }

    async fn insert<T>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: Option<i64>,
        field_ttl_secs: Option<i64>,
    ) -> Result<Option<i64>, Error>
    where
        T: Send + Sync + Serialize,
    {
        insert_update(
            Arc::clone(&self.client),
            vec![session_id],
            field,
            value,
            key_ttl_secs,
            field_ttl_secs,
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
        key_ttl_secs: Option<i64>,
        field_ttl_secs: Option<i64>,
    ) -> Result<Option<i64>, Error>
    where
        T: Send + Sync + Serialize,
    {
        insert_update(
            Arc::clone(&self.client),
            vec![session_id],
            field,
            value,
            key_ttl_secs,
            field_ttl_secs,
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
        key_ttl_secs: Option<i64>,
        field_ttl_secs: Option<i64>,
    ) -> Result<Option<i64>, Error>
    where
        T: Send + Sync + Serialize,
    {
        insert_update(
            Arc::clone(&self.client),
            vec![old_session_id, new_session_id],
            field,
            value,
            key_ttl_secs,
            field_ttl_secs,
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
        key_ttl_secs: Option<i64>,
        field_ttl_secs: Option<i64>,
    ) -> Result<Option<i64>, Error>
    where
        T: Send + Sync + Serialize,
    {
        insert_update(
            Arc::clone(&self.client),
            vec![old_session_id, new_session_id],
            field,
            value,
            key_ttl_secs,
            field_ttl_secs,
            &UPDATE_WITH_RENAME_SCRIPT_HASH,
            UPDATE_WITH_RENAME_SCRIPT,
        )
        .await
    }

    async fn rename_session_id(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
    ) -> Result<bool, Error> {
        let renamed: bool = self
            .client
            .renamenx(old_session_id, new_session_id).await?;

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

#[allow(clippy::too_many_arguments)]
async fn insert_update<C, T>(
    client: Arc<C>,
    session_ids: Vec<&Id>,
    field: &str,
    value: &T,
    key_ttl_secs: Option<i64>,
    field_ttl_secs: Option<i64>,
    once_cell: &OnceCell<String>,
    script: &str,
) -> Result<Option<i64>, Error>
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

    let result: i64 = client
        .evalsha(
            hash,
            session_ids,
            vec![
                field.as_bytes(),
                &serialized_value,
                key_ttl_secs.unwrap_or(-1).to_string().as_bytes(),
                field_ttl_secs.unwrap_or(-1).to_string().as_bytes(),
            ],
        )
        .await?;

    if result >= 0 {
        Ok(Some(result))
    } else {
        Ok(None)
    }
}

#[cfg(feature = "layered-store")]
impl<C> crate::store::LayeredHotStore for RedisStore<C>
where
    C: HashesInterface + KeysInterface + LuaInterface + Clone + Send + Sync + 'static,
{
    async fn update_many(
        &self,
        session_id: &Id,
        pairs: &[(String, Vec<u8>, Option<i64>)],
    ) -> Result<bool, Error> {
        if pairs.is_empty() {
            return Ok(true);
        }

        let hash = UPDATE_MANY_SCRIPT_HASH
            .get_or_try_init(|| async {
                let hash = fred::util::sha1_hash(UPDATE_MANY_SCRIPT);
                if !self.client.script_exists::<bool, _>(&hash).await? {
                    let _: () = self.client.script_load(UPDATE_MANY_SCRIPT).await?;
                }
                Ok::<String, fred::error::Error>(hash)
            })
            .await?;

        let mut args: Vec<Value> = Vec::with_capacity(pairs.len() * 3);

        for (field, value, field_expiry) in pairs {
            args.push(field.into());
            args.push(value.as_slice().into());
            // -1 is interpreted as persistent fields
            args.push(field_expiry.unwrap_or(-1).into());
        }

        let updated: bool = self.client.evalsha(hash, vec![session_id], args).await?;

        Ok(updated)
    }
}
