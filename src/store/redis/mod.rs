mod lua;

use crate::Id;
use crate::store::redis::lua::{
    REMOVE_SCRIPT, SET_AND_RENAME_SCRIPT, SET_MULTIPLE_SCRIPT, SET_SCRIPT,
};
use crate::store::{Error, SessionMap, SessionStore, deserialize_value, serialize_value};
use fred::clients::Pool;
use fred::interfaces::{HashesInterface, KeysInterface, LuaInterface};
use fred::types::scripts::Script;
use serde::{Serialize, de::DeserializeOwned};
use std::collections::HashMap;
use std::{fmt::Debug, sync::Arc};

#[cfg(feature = "layered-store")]
use fred::types::Value;

/// A redis session store implementation.
///
/// It uses a Redis Hash to manage session data
///
/// # Redis Version Requirements
///
/// This implementation uses Redis 7.4+ features for field-level expiration [HEXPIRE](https://redis.io/docs/latest/commands/hexpire/).
/// If you're using an earlier Redis version, field expiration will not work.
#[derive(Clone, Debug)]
pub struct RedisStore<C: HashesInterface + KeysInterface + LuaInterface + Send + Sync = Pool> {
    client: Arc<C>,
}

impl<C> RedisStore<C>
where
    C: HashesInterface + KeysInterface + LuaInterface + Send + Sync,
{
    pub async fn new(client: Arc<C>) -> Result<Self, Error> {
        load_scripts(&*client).await?;
        Ok(Self { client })
    }

    /// Reloads all Lua scripts into the Redis server's script cache.
    ///
    /// Scripts are loaded once during [`RedisStore::new`]. Call this method to
    /// reload them if the server's script cache has been lost — for example,
    /// after a Redis restart, failover to a replica, or `SCRIPT FLUSH`.
    pub async fn reload_scripts(&self) -> Result<(), Error> {
        load_scripts(&*self.client).await.map_err(Into::into)
    }
}

async fn load_scripts<C>(client: &C) -> Result<(), Error>
where
    C: HashesInterface + KeysInterface + LuaInterface + Send + Sync,
{
    for script in [
        &*SET_SCRIPT,
        &*SET_AND_RENAME_SCRIPT,
        &*REMOVE_SCRIPT,
        &*SET_MULTIPLE_SCRIPT,
    ] {
        if let Some(lua) = script.lua() {
            let _: () = client.script_load_cluster(lua.clone()).await?;
        }
    }
    Ok(())
}

impl<C> SessionStore for RedisStore<C>
where
    C: HashesInterface + KeysInterface + LuaInterface + Send + Sync + 'static,
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
        if result.len() == 0 {
            return Ok(None);
        }

        let mut map = HashMap::with_capacity(result.len());
        result.into_iter().for_each(|(field, value)| {
            map.insert(field, value);
        });

        Ok(Some(SessionMap::new(map)))
    }

    async fn set<T>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: i64,
        field_ttl_secs: i64,
        #[cfg(feature = "layered-store")] _: Option<i64>,
        #[cfg(not(feature = "layered-store"))] _: Option<std::marker::PhantomData<()>>,
    ) -> Result<i64, Error>
    where
        T: Send + Sync + Serialize,
    {
        insert_update(
            &*self.client,
            vec![session_id],
            field,
            value,
            key_ttl_secs,
            field_ttl_secs,
            &SET_SCRIPT,
        )
        .await
    }

    async fn set_and_rename<T>(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: i64,
        field_ttl_secs: i64,
        #[cfg(feature = "layered-store")] _: Option<i64>,
        #[cfg(not(feature = "layered-store"))] _: Option<std::marker::PhantomData<()>>,
    ) -> Result<i64, Error>
    where
        T: Send + Sync + Serialize,
    {
        insert_update(
            &*self.client,
            vec![old_session_id, new_session_id],
            field,
            value,
            key_ttl_secs,
            field_ttl_secs,
            &SET_AND_RENAME_SCRIPT,
        )
        .await
    }

    async fn rename_session_id(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
    ) -> Result<bool, Error> {
        self.client
            .renamenx(old_session_id, new_session_id)
            .await
            .map_err(Into::into)
    }

    async fn remove(&self, session_id: &Id, field: &str) -> Result<i64, Error> {
        REMOVE_SCRIPT
            .evalsha(&*self.client, vec![session_id], field)
            .await
            .map_err(Into::into)
    }

    async fn delete(&self, session_id: &Id) -> Result<bool, Error> {
        self.client.del(session_id).await.map_err(Into::into)
    }

    async fn expire(&self, session_id: &Id, seconds: i64) -> Result<bool, Error> {
        self.client
            .expire(session_id, seconds, None)
            .await
            .map_err(Into::into)
    }
}

#[allow(clippy::too_many_arguments)]
async fn insert_update<C, T>(
    client: &C,
    session_ids: Vec<&Id>,
    field: &str,
    value: &T,
    key_ttl_secs: i64,
    field_ttl_secs: i64,
    script: &'static std::sync::LazyLock<Script>,
) -> Result<i64, Error>
where
    C: LuaInterface + Send + Sync,
    T: Send + Sync + Serialize,
{
    let serialized_value = serialize_value(value)?;
    script
        .evalsha(
            client,
            session_ids,
            (
                field,
                serialized_value.as_slice(),
                key_ttl_secs,
                field_ttl_secs,
            ),
        )
        .await
        .map_err(Into::into)
}

#[cfg(feature = "layered-store")]
impl<C> crate::store::LayeredHotStore for RedisStore<C>
where
    C: HashesInterface + KeysInterface + LuaInterface + Send + Sync + 'static,
{
    async fn set_multiple(
        &self,
        session_id: &Id,
        pairs: &[(&str, &[u8], Option<i64>)],
    ) -> Result<i64, Error> {
        if pairs.is_empty() {
            return Ok(-2);
        }

        let mut args: Vec<Value> = Vec::with_capacity(pairs.len() * 3);

        for (field, value, ttl) in pairs {
            args.push((*field).into());
            args.push((*value).into());
            args.push(ttl.map(|n| Value::Integer(n)).unwrap_or(Value::Null))
        }

        let updated: i64 = SET_MULTIPLE_SCRIPT
            .evalsha(&*self.client, vec![session_id], args)
            .await?;

        Ok(updated)
    }
}
