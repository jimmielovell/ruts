use crate::Id;
use crate::store::{Error, LayeredColdStore, LayeredHotStore, SessionMap, SessionStore};
use serde::{Serialize, Serializer, de::DeserializeOwned};

/// An enum for explicit control over the write strategy for a
/// specific `insert` or `update` operation.
///
/// It is passed as the `value` parameter to the session methods and will be `downcast_ref`ed
/// to unwrap the inner value `T`.
/// The `i64` allows you to specify a separate, often shorter, time-to-live (TTL)
/// in seconds exclusively for the hot cache.
pub struct LayeredWriteStrategy<T: Send + Sync + Serialize + 'static>(pub T, pub i64);

impl<T: Send + Sync + Serialize + 'static> Serialize for LayeredWriteStrategy<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

/// [`LayeredStore`], a composite store that layers a fast,
/// ephemeral "hot" cache (like Redis) on top of a slower, persistent "cold"
/// store (like Postgres). It is designed for scenarios where sessions can have
/// long lifespans but should only occupy expensive cache memory when actively
/// being used thus balancing performance and durability.
///
/// ## Core Strategies
///
/// - **Cache-Aside Reads**: On a read operation (`get`, `get_all`), the store
///   first checks the hot cache. If the data is present, it is
///   returned immediately. If not, the store queries the cold
///   store, and if the data is found, it "warms" the hot cache by populating it
///   with the data before returning it if during `insert`/`update`
///   `LayeredWriteStrategy::WriteThrough`(default) was the strategy used.
///
/// - **Write-Through (Default)**: By default, write operations (`insert`, `update`)
///   are written to both the hot and cold stores simultaneously. This guarantees
///   data consistency.
///
/// ## Fine-Grained Write Control
///
/// The default write-through behavior can be overridden on a per-call basis
/// using the [`LayeredWriteStrategy`] enum. This gives you precise control over
/// where your session data is stored, which is especially useful for managing
/// cache resources.
///
/// The enum allows you to:
/// 1.  Write to the hot cache only.
/// 2.  Write to the cold store only.
/// 3.  Write through to both, but with a specific, shorter TTL for the hot cache.
///
/// ## Example
///
/// ```rust,no_run
/// use ruts::store::layered::LayeredWriteStrategy;
/// use ruts::Session;
/// # use ruts::store::redis::RedisStore;
/// # use ruts::store::postgres::PostgresStore;
/// # use ruts::store::layered::LayeredStore;
/// # type MySession = Session<LayeredStore<RedisStore, PostgresStore>>;
/// # #[derive(serde::Serialize)]
/// # struct User { id: i32 }
/// # async fn handler(session: MySession) {
/// # let user = User { id: 1 };
///
/// let long_term_expiry = 60  * 60 * 24 * 30; // valid for 1 month
///
/// // However, we only want it to live in the hot cache (Redis) for 1 hour.
/// let short_term_hot_cache_expiry = 60 * 60;
///
/// let strategy = LayeredWriteStrategy(
///     user,
///     short_term_hot_cache_expiry,
/// );
///
/// // The cold store (Postgres) will get the long-term expiry,
/// // but the hot store (Redis) will be capped at the shorter TTL.
/// session.update("user", &strategy, Some(long_term_expiry)).await.unwrap();
/// # }
/// ```
#[derive(Clone, Debug)]
pub struct LayeredStore<Hot, Cold>
where
    Hot: SessionStore + LayeredHotStore,
    Cold: SessionStore,
{
    hot: Hot,
    cold: Cold,
}

impl<Hot, Cold> LayeredStore<Hot, Cold>
where
    Hot: SessionStore + LayeredHotStore,
    Cold: SessionStore + LayeredColdStore,
{
    /// Creates a new `LayeredStore`.
    ///
    /// # Arguments
    ///
    /// * `hot` - The fast cache store (e.g., `RedisStore`).
    /// * `cold` - The persistent source of truth (e.g., `PostgresStore`).
    pub fn new(hot: Hot, cold: Cold) -> Self {
        Self { hot, cold }
    }
}

impl<Hot, Cold> SessionStore for LayeredStore<Hot, Cold>
where
    Hot: SessionStore + LayeredHotStore,
    Cold: SessionStore + LayeredColdStore,
{
    async fn get<T>(&self, session_id: &Id, field: &str) -> Result<Option<T>, Error>
    where
        T: Send + Sync + DeserializeOwned,
    {
        match self.hot.get(session_id, field).await? {
            Some(value) => Ok(Some(value)),
            None => match self.cold.get_all_with_meta(session_id).await? {
                Some((session_map, hot_cache_ttl_map)) => {
                    let pairs_to_cache: Vec<(&str, &[u8], Option<i64>)> = session_map
                        .iter()
                        .filter_map(|(key, value)| {
                            let hot_cache_ttl = hot_cache_ttl_map.get(key).unwrap().to_owned();
                            if hot_cache_ttl != Some(0) {
                                Some((key.as_str(), value.as_slice(), hot_cache_ttl))
                            } else {
                                None
                            }
                        })
                        .collect();

                    if !pairs_to_cache.is_empty() {
                        self.hot.update_many(session_id, &pairs_to_cache).await?;
                    }

                    session_map.get(field)
                }
                None => Ok(None),
            },
        }
    }

    async fn get_all(&self, session_id: &Id) -> Result<Option<SessionMap>, Error> {
        match self.hot.get_all(session_id).await? {
            Some(session_map) => Ok(Some(session_map)),
            None => match self.cold.get_all_with_meta(session_id).await? {
                Some((session_map, hot_cache_ttl_map)) => {
                    let pairs_to_cache: Vec<(&str, &[u8], Option<i64>)> = session_map
                        .iter()
                        .filter_map(|(key, value)| {
                            let hot_cache_ttl = hot_cache_ttl_map.get(key).unwrap().to_owned();
                            if hot_cache_ttl != Some(0) {
                                Some((key.as_str(), value.as_slice(), hot_cache_ttl))
                            } else {
                                None
                            }
                        })
                        .collect();

                    if !pairs_to_cache.is_empty() {
                        self.hot.update_many(session_id, &pairs_to_cache).await?;
                    }

                    Ok(Some(session_map))
                }
                None => Ok(None),
            },
        }
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
        T: Send + Sync + Serialize + 'static,
    {
        let mut value = value;
        let mut hot_cache_ttl = field_ttl_secs;

        if let Some(strategy) =
            (value as &dyn std::any::Any).downcast_ref::<LayeredWriteStrategy<T>>()
        {
            value = &strategy.0;
            hot_cache_ttl = Some(strategy.1);
        }

        let (_, cold_ttl) = tokio::try_join!(
            self.hot
                .insert(session_id, field, value, hot_cache_ttl, hot_cache_ttl),
            self.cold.insert_with_meta(
                session_id,
                field,
                value,
                key_ttl_secs,
                field_ttl_secs,
                hot_cache_ttl
            ),
        )?;

        Ok(cold_ttl)
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
        T: Send + Sync + Serialize + 'static,
    {
        let mut value = value;
        let mut hot_cache_ttl = field_ttl_secs;

        if let Some(strategy) =
            (value as &dyn std::any::Any).downcast_ref::<LayeredWriteStrategy<T>>()
        {
            value = &strategy.0;
            hot_cache_ttl = Some(strategy.1);
        }

        let (_, cold_ttl) = tokio::try_join!(
            self.hot
                .update(session_id, field, value, hot_cache_ttl, hot_cache_ttl),
            self.cold.update_with_meta(
                session_id,
                field,
                value,
                key_ttl_secs,
                field_ttl_secs,
                hot_cache_ttl
            ),
        )?;

        Ok(cold_ttl)
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
        T: Send + Sync + Serialize + 'static,
    {
        let mut value = value;
        let mut hot_cache_ttl = field_ttl_secs;

        if let Some(strategy) =
            (value as &dyn std::any::Any).downcast_ref::<LayeredWriteStrategy<T>>()
        {
            value = &strategy.0;
            hot_cache_ttl = Some(strategy.1);
        }

        let (_, cold_ttl) = tokio::try_join!(
            self.hot.insert_with_rename(
                old_session_id,
                new_session_id,
                field,
                value,
                hot_cache_ttl,
                hot_cache_ttl,
            ),
            self.cold.insert_with_rename_with_meta(
                old_session_id,
                new_session_id,
                field,
                value,
                key_ttl_secs,
                field_ttl_secs,
                hot_cache_ttl
            ),
        )?;

        Ok(cold_ttl)
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
        T: Send + Sync + Serialize + 'static,
    {
        let mut value = value;
        let mut hot_cache_ttl = field_ttl_secs;

        if let Some(strategy) =
            (value as &dyn std::any::Any).downcast_ref::<LayeredWriteStrategy<T>>()
        {
            value = &strategy.0;
            hot_cache_ttl = Some(strategy.1);
        }

        let (_, cold_ttl) = tokio::try_join!(
            self.hot.update_with_rename(
                old_session_id,
                new_session_id,
                field,
                value,
                hot_cache_ttl,
                hot_cache_ttl,
            ),
            self.cold.update_with_rename_with_meta(
                old_session_id,
                new_session_id,
                field,
                value,
                key_ttl_secs,
                field_ttl_secs,
                hot_cache_ttl
            ),
        )?;

        Ok(cold_ttl)
    }

    async fn rename_session_id(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
    ) -> Result<bool, Error> {
        let (hot_result, cold_result) = tokio::try_join!(
            self.hot.rename_session_id(old_session_id, new_session_id),
            self.cold.rename_session_id(old_session_id, new_session_id),
        )?;
        Ok(hot_result && cold_result)
    }

    async fn remove(&self, session_id: &Id, field: &str) -> Result<i64, Error> {
        let (_, cold_ttl) = tokio::try_join!(
            self.hot.remove(session_id, field),
            self.cold.remove(session_id, field),
        )?;

        Ok(cold_ttl)
    }

    async fn delete(&self, session_id: &Id) -> Result<bool, Error> {
        let (hot_deleted, cold_deleted) =
            tokio::try_join!(self.hot.delete(session_id), self.cold.delete(session_id),)?;
        Ok(hot_deleted && cold_deleted)
    }

    async fn expire(&self, session_id: &Id, seconds: i64) -> Result<bool, Error> {
        let (hot_expired, cold_expired) = tokio::try_join!(
            self.hot.expire(session_id, seconds),
            self.cold.expire(session_id, seconds),
        )?;
        Ok(hot_expired && cold_expired)
    }
}


#[cfg(test)]
mod tests {
    #![cfg(all(feature = "redis-store", feature = "postgres-store"))]

    use fred::{clients::Client, interfaces::*};
    use super::*;
    use sqlx::PgPool;
    use std::{sync::Arc, time::Duration};
    use serde::Deserialize;
    use crate::store::postgres::{PostgresStore, PostgresStoreBuilder};
    use crate::store::redis::RedisStore;

    #[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
    struct TestUser {
        pub id: i64,
        pub name: String,
    }

    fn create_test_user() -> TestUser {
        TestUser {
            id: 1,
            name: "Test User".to_string(),
        }
    }

    async fn setup_store() -> LayeredStore<RedisStore<Client>, PostgresStore> {
        let database_url =
            std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for tests");
        let pool = PgPool::connect(&database_url).await.unwrap();

        sqlx::query("drop table if exists sessions")
            .execute(&pool)
            .await
            .unwrap();
        let cold_store = PostgresStoreBuilder::new(pool.clone())
            .build()
            .await
            .unwrap();

        let client = Client::default();
        client.init().await.unwrap();
        let hot_store = RedisStore::new(Arc::new(client.clone()));

        LayeredStore::new(hot_store, cold_store)
    }

    #[tokio::test]
    async fn test_layered_cache_aside_flow() {
        let store = setup_store().await;
        let session_id = Id::default();
        let test_user = create_test_user();

        // 1. Write a value with a long TTL for the cold store, but a very short TTL
        //    for the hot cache. This simulates a cache entry that will expire quickly.
        let strategy = LayeredWriteStrategy(test_user.clone(), 1);
        store
            .update(&session_id, "user", &strategy, Some(3600), Some(3600))
            .await
            .unwrap();

        // 2. Immediately get the value. This should be a cache HIT.
        let user_from_hit: TestUser = store
            .get(&session_id, "user")
            .await
            .unwrap()
            .expect("Should get value from hot cache");
        assert_eq!(user_from_hit, test_user);

        // 3. Wait for the hot cache entry to expire.
        tokio::time::sleep(Duration::from_secs(2)).await;

        // 4. Get the value again. This should be a cache MISS, which triggers a
        //    read from the cold store and automatically warms the cache.
        let user_from_miss: TestUser = store
            .get(&session_id, "user")
            .await
            .unwrap()
            .expect("Should fetch from cold store after cache expiry");
        assert_eq!(user_from_miss, test_user);
    }

    #[tokio::test]
    async fn test_layered_delete() {
        let store = setup_store().await;
        let session_id = Id::default();
        let test_user = create_test_user();

        store
            .update(&session_id, "user", &test_user, Some(3600), Some(3600))
            .await
            .unwrap();
        assert!(
            store
                .get::<TestUser>(&session_id, "user")
                .await
                .unwrap()
                .is_some()
        );

        store.delete(&session_id).await.unwrap();

        assert!(
            store
                .get::<TestUser>(&session_id, "user")
                .await
                .unwrap()
                .is_none()
        );
    }
}
