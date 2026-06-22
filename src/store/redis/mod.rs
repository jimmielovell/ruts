mod lua;

use crate::Id;
use crate::store::redis::lua::{
    EXPIRE_FIELD_SCRIPT, RENAME_SCRIPT, SET_AND_RENAME_SCRIPT, SET_MULTIPLE_SCRIPT, SET_SCRIPT,
};
use crate::store::{Error, SessionMap, SessionStore, Ttl, deserialize_value, serialize_value};
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
/// It uses a Redis Hash to manage session data, natively mapping session
/// liveness to the existence of its fields.
///
/// # Redis Version Requirements
///
/// This implementation relies on Redis 7.4+ field-level expiration
/// ([HEXPIRE](https://redis.io/docs/latest/commands/hexpire/)). The session is
/// purged by Redis once its last field expires.
///
/// # Redis Cluster
///
/// `set_and_rename` and `rename_session_id` operate on two keys (`old` and
/// `new`) within a single script, so in cluster mode both ids must hash to the
/// same slot. Use a common hash tag (e.g. `{user42}:old` / `{user42}:new`) so
/// the keys co-locate; otherwise the rename will fail at runtime.
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
        &*RENAME_SCRIPT,
        &*EXPIRE_FIELD_SCRIPT,
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

        let map = match result {
            Some(map) if !map.is_empty() => map,
            _ => return Ok(None),
        };

        Ok(Some(SessionMap::new(map)))
    }

    async fn set<T>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        field_ttl: Ttl,
        #[cfg(feature = "layered-store")] _: Option<Ttl>,
        #[cfg(not(feature = "layered-store"))] _: Option<std::marker::PhantomData<()>>,
    ) -> Result<(), Error>
    where
        T: Send + Sync + Serialize,
    {
        insert_update(
            &*self.client,
            vec![session_id],
            field,
            value,
            field_ttl,
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
        field_ttl: Ttl,
        #[cfg(feature = "layered-store")] _: Option<Ttl>,
        #[cfg(not(feature = "layered-store"))] _: Option<std::marker::PhantomData<()>>,
    ) -> Result<(), Error>
    where
        T: Send + Sync + Serialize,
    {
        insert_update(
            &*self.client,
            vec![old_session_id, new_session_id],
            field,
            value,
            field_ttl,
            &SET_AND_RENAME_SCRIPT,
        )
        .await
    }

    async fn rename_session_id(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
    ) -> Result<bool, Error> {
        let result: i64 = RENAME_SCRIPT
            .evalsha(&*self.client, vec![old_session_id, new_session_id], ())
            .await?;

        Ok(result == 1)
    }

    async fn remove(&self, session_id: &Id, field: &str) -> Result<(), Error> {
        let _: () = self.client.hdel(session_id, field).await?;
        Ok(())
    }

    async fn delete(&self, session_id: &Id) -> Result<bool, Error> {
        let deleted: i64 = self.client.del(session_id).await?;
        Ok(deleted > 0)
    }

    async fn expire_field(&self, session_id: &Id, field: &str, ttl: Ttl) -> Result<bool, Error> {
        let result: i64 = EXPIRE_FIELD_SCRIPT
            .evalsha(&*self.client, vec![session_id], (field, i64::from(ttl)))
            .await?;

        Ok(result == 1)
    }
}

async fn insert_update<C, T>(
    client: &C,
    session_ids: Vec<&Id>,
    field: &str,
    value: &T,
    field_ttl: Ttl,
    script: &'static std::sync::LazyLock<Script>,
) -> Result<(), Error>
where
    C: LuaInterface + Send + Sync,
    T: Send + Sync + Serialize,
{
    let serialized_value = serialize_value(value)?;

    let _: () = script
        .evalsha(
            client,
            session_ids,
            (field, serialized_value.as_slice(), i64::from(field_ttl)),
        )
        .await?;

    Ok(())
}

#[cfg(feature = "layered-store")]
impl<C> crate::store::LayeredHotStore for RedisStore<C>
where
    C: HashesInterface + KeysInterface + LuaInterface + Send + Sync + 'static,
{
    async fn set_multiple(
        &self,
        session_id: &Id,
        pairs: &[(&str, &[u8], Ttl)],
    ) -> Result<(), Error> {
        if pairs.is_empty() {
            return Ok(());
        }

        let mut args: Vec<Value> = Vec::with_capacity(pairs.len() * 3);

        for (field, value, ttl) in pairs {
            args.push((*field).into());
            args.push((*value).into());
            args.push(Value::Integer((*ttl).into()));
        }

        let _: () = SET_MULTIPLE_SCRIPT
            .evalsha(&*self.client, vec![session_id], args)
            .await?;

        Ok(())
    }
}
