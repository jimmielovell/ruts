mod lua;

use crate::Id;
use crate::store::redis::lua::{
    INSERT_SCRIPT, INSERT_SCRIPT_HASH, INSERT_WITH_RENAME_SCRIPT, INSERT_WITH_RENAME_SCRIPT_HASH,
    REMOVE_SCRIPT, REMOVE_SCRIPT_HASH, UPDATE_MANY_SCRIPT, UPDATE_MANY_SCRIPT_HASH, UPDATE_SCRIPT,
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
    ) -> Result<i64, Error>
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
    ) -> Result<i64, Error>
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
    ) -> Result<i64, Error>
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
    ) -> Result<i64, Error>
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
        Ok(self.client.renamenx(old_session_id, new_session_id).await?)
    }

    async fn remove(&self, session_id: &Id, field: &str) -> Result<i64, Error> {
        let client = Arc::new(&self.client);

        let hash = REMOVE_SCRIPT_HASH
            .get_or_try_init(|| async {
                let hash = fred::util::sha1_hash(REMOVE_SCRIPT);
                if !client.script_exists::<bool, _>(&hash).await? {
                    let _: () = client.script_load(REMOVE_SCRIPT).await?;
                }
                Ok::<String, fred::error::Error>(hash)
            })
            .await?;

        let result: i64 = client.evalsha(hash, vec![session_id], field).await?;

        Ok(result)
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
) -> Result<i64, Error>
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
            (
                field,
                serialized_value.as_slice(),
                key_ttl_secs.unwrap_or(-2),
                field_ttl_secs.unwrap_or(-2),
            ),
        )
        .await?;

    Ok(result)
}

#[cfg(feature = "layered-store")]
impl<C> crate::store::LayeredHotStore for RedisStore<C>
where
    C: HashesInterface + KeysInterface + LuaInterface + Clone + Send + Sync + 'static,
{
    async fn update_many(
        &self,
        session_id: &Id,
        pairs: &[(&str, &[u8], Option<i64>)],
    ) -> Result<i64, Error> {
        if pairs.is_empty() {
            return Ok(-2);
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

        for (field, value, ttl) in pairs {
            args.push((*field).into());
            args.push((*value).into());
            args.push(ttl.map(|n| Value::Integer(n)).unwrap_or(Value::Null))
        }

        let updated: i64 = self.client.evalsha(hash, vec![session_id], args).await?;

        Ok(updated)
    }
}

#[cfg(test)]
mod tests {
    use fred::clients::Client;
    use fred::prelude::ClientLike;
    use super::*;
    use std::sync::Arc;
    use tokio::time::{Duration, sleep};

    async fn setup_store() -> RedisStore<Client> {
        let client = Client::default();
        client.connect();
        client.wait_for_connect().await.unwrap();
        RedisStore::new(Arc::new(client))
    }

    #[tokio::test]
    async fn test_insert_and_get() {
        let store = setup_store().await;
        let sid = Id::default();

        let ttl = store
            .insert(&sid, "field1", &"value1", Some(10), None)
            .await
            .unwrap();
        assert_eq!(ttl, 10);

        let v: Option<String> = store.get(&sid, "field1").await.unwrap();
        assert_eq!(v.unwrap(), "value1");
    }

    #[tokio::test]
    async fn test_insert_with_field_ttl() {
        let store = setup_store().await;
        let sid = Id::default();

        store
            .insert(&sid, "f", &"temp", None, Some(1))
            .await
            .unwrap();

        // Initially exists
        let v: Option<String> = store.get(&sid, "f").await.unwrap();
        assert_eq!(v.unwrap(), "temp");

        // Should expire after 1s
        sleep(Duration::from_secs(2)).await;
        let v: Option<String> = store.get(&sid, "f").await.unwrap();
        assert!(v.is_none());
    }

    #[tokio::test]
    async fn test_update_with_persistent_field() {
        let store = setup_store().await;
        let sid = Id::default();

        let ttl = store
            .insert(&sid, "f", &"x", Some(5), Some(5))
            .await
            .unwrap();
        assert_eq!(ttl, 5);

        let ttl = store.update(&sid, "f", &"y", None, Some(-1)).await.unwrap();
        assert_eq!(ttl, -1);
    }

    #[tokio::test]
    async fn test_insert_with_rename_success() {
        let store = setup_store().await;
        let old_sid = Id::default();
        let new_sid = Id::default();

        store
            .insert(&old_sid, "f", &"foo", None, None)
            .await
            .unwrap();
        store
            .insert_with_rename(&old_sid, &new_sid, "g", &"bar", None, None)
            .await
            .unwrap();

        let v: Option<String> = store.get(&new_sid, "f").await.unwrap();
        assert_eq!(v.unwrap(), "foo");

        let v: Option<String> = store.get(&new_sid, "g").await.unwrap();
        assert_eq!(v.unwrap(), "bar");
    }

    #[tokio::test]
    async fn test_insert_with_rename_conflict() {
        let store = setup_store().await;
        let old_sid = Id::default();
        let new_sid = Id::default();

        store
            .insert(&old_sid, "f", &"foo", None, None)
            .await
            .unwrap();
        store
            .insert(&new_sid, "existing", &"bar", None, None)
            .await
            .unwrap();

        // RENAMENX should fail, but we still insert into new_sid
        store
            .insert_with_rename(&old_sid, &new_sid, "g", &"baz", None, None)
            .await
            .unwrap();

        let v: Option<String> = store.get(&new_sid, "g").await.unwrap();
        assert_eq!(v.unwrap(), "baz");
    }

    #[tokio::test]
    async fn test_remove_and_delete() {
        let store = setup_store().await;
        let sid = Id::default();

        store
            .insert(&sid, "f1", &"v1", Some(10), None)
            .await
            .unwrap();
        store
            .insert(&sid, "f2", &"v2", Some(10), None)
            .await
            .unwrap();

        let ttl = store.remove(&sid, "f1").await.unwrap();
        assert_eq!(ttl, 10); // still has key

        let ttl = store.remove(&sid, "f2").await.unwrap();
        assert_eq!(ttl, -2);
    }

    #[cfg(feature = "layered-store")]
    #[tokio::test]
    async fn test_update_many() {
        use crate::store::{LayeredHotStore, serialize_value};
        
        let store = setup_store().await;
        let sid = Id::default();

        let a_val = serialize_value(&"1").unwrap();
        let b_val = serialize_value(&"2").unwrap();

        let pairs = vec![
            ("a", a_val.as_slice(), Some(3)),
            ("b", b_val.as_slice(), Some(-1)), // persistent
        ];

        let ttl = store.update_many(&sid, pairs.as_slice()).await.unwrap();
        assert_eq!(ttl, -1); // persistent because of "b"

        let v: Option<String> = store.get(&sid, "a").await.unwrap();
        assert_eq!(v.unwrap(), "1");

        let v: Option<String> = store.get(&sid, "b").await.unwrap();
        assert_eq!(v.unwrap(), "2");
    }
}

