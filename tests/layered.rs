#![cfg(feature = "layered-store")]

mod common;

#[cfg(test)]
mod tests {
    use super::common::{create_test_session, TestPreferences, TestSession, TestUser};
    use fred::{clients::Client, interfaces::*};
    use ruts::{
        store::{
            layered::{LayeredStore, LayeredWriteStrategy},
            postgres::{PostgresStore, PostgresStoreBuilder},
            redis::RedisStore,
            SessionStore,
        },
        Id,
    };
    use sqlx::PgPool;
    use std::{sync::Arc, time::Duration};

    /// Sets up a full layered store with isolated, clean Redis and Postgres backends.
    /// This function only returns the store itself to ensure tests treat it as a black box.
    async fn setup_layered_store() -> LayeredStore<RedisStore<Client>, PostgresStore> {
        // --- Postgres (Cold Store) Setup ---
        let database_url =
            std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for tests");
        let pool = PgPool::connect(&database_url).await.unwrap();
        let table_name = format!("sessions_{}", rand::random::<u32>());
        // Ensure the table is clean for every test run.
        sqlx::query(&format!("DROP TABLE IF EXISTS {}", table_name))
            .execute(&pool)
            .await
            .unwrap();
        let cold_store = PostgresStoreBuilder::new(pool.clone())
            .table_name(table_name)
            .build()
            .await
            .unwrap();

        // --- Redis (Hot Store) Setup ---
        let client = Client::default();
        client.init().await.unwrap();
        // client.flushall(false).await.unwrap(); // Ensure clean Redis DB
        let hot_store = RedisStore::new(Arc::new(client.clone()));

        // --- Layered Store ---
        LayeredStore::new(hot_store, cold_store)
    }

    #[tokio::test]
    async fn test_layered_cache_aside_flow() {
        let store = setup_layered_store().await;
        let session_id = Id::default();
        let test_user = create_test_session().user;

        // 1. Write a value with a long TTL for the cold store, but a very short TTL
        //    for the hot cache. This simulates a cache entry that will expire quickly.
        let strategy = LayeredWriteStrategy::WriteThrough(test_user.clone(), Some(1));
        store
            .update(&session_id, "user", &strategy, 3600, None)
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
    async fn test_layered_write_strategies() {
        let store = setup_layered_store().await;
        let session_id = Id::default();
        let test_session = create_test_session();

        // --- Strategy 1: HotCache Only ---
        // Prove it by writing with a short TTL and showing it disappears completely.
        let hot_only_strategy = LayeredWriteStrategy::HotCache(test_session.user.clone());
        store
            .update(&session_id, "user_hot", &hot_only_strategy, 1, None)
            .await
            .unwrap();

        // It exists immediately.
        assert!(store
            .get::<TestUser>(&session_id, "user_hot")
            .await
            .unwrap()
            .is_some());

        // After TTL, it's gone from everywhere because it was never persisted.
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert!(store
            .get::<TestUser>(&session_id, "user_hot")
            .await
            .unwrap()
            .is_none());

        // --- Strategy 2: ColdCache Only ---
        // Prove it by showing it's retrievable even after the hot cache is cleared.
        let cold_only_strategy = LayeredWriteStrategy::ColdCache(test_session.preferences.clone());
        store
            .update(&session_id, "prefs_cold", &cold_only_strategy, 3600, None)
            .await
            .unwrap();

        // To prove it wasn't in the hot cache, we write a temporary hot-cache-only value
        // with a short TTL. This effectively creates a session key in Redis we can wait to expire.
        store
            .update(
                &session_id,
                "temp",
                &LayeredWriteStrategy::HotCache("...".to_string()),
                1,
                None,
            )
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await; // Wait for Redis session to expire.

        // Now, getting the cold-only value should still succeed, triggering a cache warm.
        assert!(store
            .get::<TestPreferences>(&session_id, "prefs_cold")
            .await
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn test_layered_delete() {
        let store = setup_layered_store().await;
        let session_id = Id::default();
        let test_user = create_test_session().user;

        // 1. Write a value to both stores.
        store
            .update(&session_id, "user", &test_user, 3600, None)
            .await
            .unwrap();
        // Verify it's retrievable.
        assert!(store
            .get::<TestUser>(&session_id, "user")
            .await
            .unwrap()
            .is_some());

        // 2. Delete the session.
        store.delete(&session_id).await.unwrap();

        // 3. Assert it's gone by trying to retrieve it.
        assert!(store
            .get::<TestUser>(&session_id, "user")
            .await
            .unwrap()
            .is_none());
    }
}
