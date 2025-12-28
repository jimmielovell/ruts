use crate::Id;
use crate::store::{Error, LayeredColdStore, LayeredHotStore, SessionMap, SessionStore};
use serde::{Serialize, de::DeserializeOwned};

/// [`LayeredStore`], a composite store that layers a fast,
/// ephemeral "hot" cache (like Redis) on top of a slower, persistent "cold"
/// store (like Postgres). It is designed for scenarios where sessions can have
/// long lifespans but should only occupy expensive cache memory when actively
/// being used thus balancing performance and durability.
///
/// ## Example
///
/// ```rust,no_run
/// # use ruts::Session;
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
/// // The cold store (Postgres) will get the long-term expiry,
/// // but the hot store (Redis) will be capped at the shorter TTL.
/// session.set("user", &user, Some(long_term_expiry), Some(short_term_hot_cache_expiry))
///     .await
///     .unwrap();
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
                        self.hot.set_multiple(session_id, &pairs_to_cache).await?;
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
                        self.hot.set_multiple(session_id, &pairs_to_cache).await?;
                    }

                    Ok(Some(session_map))
                }
                None => Ok(None),
            },
        }
    }

    async fn set<T>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: Option<i64>,
        field_ttl_secs: Option<i64>,
        hot_cache_ttl_secs: Option<i64>,
    ) -> Result<i64, Error>
    where
        T: Send + Sync + Serialize + 'static,
    {
        let hot_cache_ttl = hot_cache_ttl_secs.or(field_ttl_secs);
        let (_, cold_ttl) = tokio::try_join!(
            self.hot
                .set(session_id, field, value, hot_cache_ttl, hot_cache_ttl, None),
            self.cold.set_with_meta(
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

    async fn set_and_rename<T>(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: Option<i64>,
        field_ttl_secs: Option<i64>,
        hot_cache_ttl_secs: Option<i64>,
    ) -> Result<i64, Error>
    where
        T: Send + Sync + Serialize + 'static,
    {
        let hot_cache_ttl = hot_cache_ttl_secs.or(field_ttl_secs);
        let (_, cold_ttl) = tokio::try_join!(
            self.hot.set_and_rename(
                old_session_id,
                new_session_id,
                field,
                value,
                hot_cache_ttl,
                hot_cache_ttl,
                None
            ),
            self.cold.set_and_rename_with_meta(
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

    use super::*;
    use crate::store::postgres::{PostgresStore, PostgresStoreBuilder};
    use crate::store::redis::RedisStore;
    use fred::{clients::Client, interfaces::*};
    use serde::Deserialize;
    use sqlx::PgPool;
    use std::{sync::Arc, time::Duration};

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
        let cold_store = PostgresStoreBuilder::new(pool.clone(), true)
            .build()
            .await
            .unwrap();

        let client = Client::default();
        client.init().await.unwrap();
        let hot_store = RedisStore::new(Arc::new(client.clone()));

        LayeredStore::new(hot_store, cold_store)
    }

    #[tokio::test]
    async fn test_layered_cache() {
        let store = setup_store().await;
        let session_id = Id::default();
        let test_user = create_test_user();

        store
            .set(
                &session_id,
                "user",
                &test_user,
                Some(3600),
                Some(3600),
                Some(1),
            )
            .await
            .unwrap();

        // Immediately get the value. This should be a cache HIT.
        let user_from_hit: TestUser = store
            .get(&session_id, "user")
            .await
            .unwrap()
            .expect("Should get value from hot cache");
        assert_eq!(user_from_hit, test_user);

        // Wait for the hot cache entry to expire.
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Get the value again. This should be a cache MISS, which triggers a
        // read from the cold store and automatically warms the cache.
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
            .set(
                &session_id,
                "user",
                &test_user,
                Some(3600),
                Some(3600),
                None,
            )
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
