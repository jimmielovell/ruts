#![cfg(feature = "layered-store")]

mod common;

#[cfg(test)]
mod tests {
    use super::common::{TestPreferences, TestUser, create_test_session};
    use fred::{clients::Client, interfaces::*};
    use ruts::{
        Id,
        store::{
            SessionStore,
            layered::{LayeredStore, LayeredWriteStrategy},
            postgres::{PostgresStore, PostgresStoreBuilder},
            redis::RedisStore,
        },
    };
    use sqlx::PgPool;
    use std::{sync::Arc, time::Duration};

    async fn setup_layered_store() -> LayeredStore<RedisStore<Client>, PostgresStore> {
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

        let client = Client::default();
        client.init().await.unwrap();
        let hot_store = RedisStore::new(Arc::new(client.clone()));

        LayeredStore::new(hot_store, cold_store)
    }

    #[tokio::test]
    async fn test_layered_cache_aside_flow() {
        let store = setup_layered_store().await;
        let session_id = Id::default();
        let test_user = create_test_session().user;

        // 1. Write a value with a long TTL for the cold store, but a very short TTL
        //    for the hot cache. This simulates a cache entry that will expire quickly.
        let strategy = LayeredWriteStrategy::WriteThrough(test_user.clone(), 1);
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
    async fn test_layered_write_strategies() {
        let store = setup_layered_store().await;
        let session_id = Id::default();
        let test_session = create_test_session();

        let hot_only_strategy = LayeredWriteStrategy::HotCache(test_session.user.clone());
        store
            .update(&session_id, "user_hot", &hot_only_strategy, Some(1), Some(1))
            .await
            .unwrap();

        assert!(
            store
                .get::<TestUser>(&session_id, "user_hot")
                .await
                .unwrap()
                .is_some()
        );

        tokio::time::sleep(Duration::from_secs(2)).await;
        assert!(
            store
                .get::<TestUser>(&session_id, "user_hot")
                .await
                .unwrap()
                .is_none()
        );

        let cold_only_strategy = LayeredWriteStrategy::ColdCache(test_session.preferences.clone());
        store
            .update(&session_id, "prefs_cold", &cold_only_strategy, Some(3600), Some(3600))
            .await
            .unwrap();

        store
            .update(
                &session_id,
                "temp",
                &LayeredWriteStrategy::HotCache("...".to_string()),
                Some(1),
                Some(1),
            )
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await; // Wait for Redis session to expire.

        assert!(
            store
                .get::<TestPreferences>(&session_id, "prefs_cold")
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn test_layered_delete() {
        let store = setup_layered_store().await;
        let session_id = Id::default();
        let test_user = create_test_session().user;

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
