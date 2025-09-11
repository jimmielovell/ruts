#![cfg(feature = "postgres-store")]

#[cfg(test)]
mod tests {
    use ruts::Id;
    use ruts::store::SessionStore;
    use ruts::store::postgres::{PostgresStore, PostgresStoreBuilder};
    use serde::{Deserialize, Serialize};
    use sqlx::PgPool;
    use std::sync::Arc;

    #[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
    struct TestData {
        value: String,
    }

    async fn setup_store() -> Arc<PostgresStore> {
        let database_url =
            std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for tests");
        let pool = PgPool::connect(&database_url).await.unwrap();

        // Clean up table before each test run
        sqlx::query("drop table if exists sessions")
            .execute(&pool)
            .await
            .unwrap();

        let store = PostgresStoreBuilder::new(pool.clone())
            .build()
            .await
            .unwrap();
        Arc::new(store)
    }

    // --- Basic Insert/Get/Update ---
    #[tokio::test]
    async fn test_insert_and_get() {
        let store = setup_store().await;
        let session_id = Id::default();
        let field = "field1";
        let value = TestData { value: "hello".into() };

        let ttl = store.insert(&session_id, field, &value, Some(60), Some(60)).await.unwrap();
        assert!(ttl > 55);

        let fetched: Option<TestData> = store.get(&session_id, field).await.unwrap();
        assert_eq!(fetched, Some(value.clone()));

        // Insert again shouldn't overwrite
        store.insert(&session_id, field, &TestData { value: "x".into() }, Some(60), Some(60))
            .await.unwrap();
        let fetched2: Option<TestData> = store.get(&session_id, field).await.unwrap();
        assert_eq!(fetched2, Some(value));
    }

    #[tokio::test]
    async fn test_update_overwrites() {
        let store = setup_store().await;
        let session_id = Id::default();
        let field = "field_update";
        let value = TestData { value: "initial".into() };
        let updated_value = TestData { value: "updated".into() };

        store.insert(&session_id, field, &value, Some(60), Some(60)).await.unwrap();
        store.update(&session_id, field, &updated_value, Some(60), Some(60)).await.unwrap();

        let fetched: Option<TestData> = store.get(&session_id, field).await.unwrap();
        assert_eq!(fetched, Some(updated_value));
    }

    // --- TTL behaviors ---
    #[tokio::test]
    async fn test_ttl_zero_removes() {
        let store = setup_store().await;
        let session_id = Id::default();
        let field = "ttl_zero";

        // Field TTL = 0 triggers removal
        let ttl = store.insert(&session_id, field, &TestData { value: "x".into() }, Some(60), Some(0))
            .await.unwrap();
        assert_eq!(ttl, -2);

        let fetched: Option<TestData> = store.get(&session_id, field).await.unwrap();
        assert!(fetched.is_none());
    }

    #[tokio::test]
    async fn test_ttl_negative_persists() {
        let store = setup_store().await;
        let session_id = Id::default();
        let field = "ttl_neg";

        let ttl = store.insert(&session_id, field, &TestData { value: "y".into() }, Some(60), Some(-1))
            .await.unwrap();
        assert_eq!(ttl, -1);

        let fetched: Option<TestData> = store.get(&session_id, field).await.unwrap();
        assert!(fetched.is_some());
    }

    #[tokio::test]
    async fn test_expire_method() {
        let store = setup_store().await;
        let session_id = Id::default();
        let field = "expire_field";
        store.insert(&session_id, field, &TestData { value: "temp".into() }, Some(2), Some(2))
            .await.unwrap();

        tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
        let fetched: Option<TestData> = store.get(&session_id, field).await.unwrap();
        assert!(fetched.is_none());
    }

    // --- Remove/Delete ---
    #[tokio::test]
    async fn test_remove_and_delete() {
        let store = setup_store().await;
        let session_id = Id::default();
        let field = "to_remove";

        store.insert(&session_id, field, &TestData { value: "bye".into() }, Some(60), Some(60))
            .await.unwrap();
        let ttl = store.remove(&session_id, field).await.unwrap();
        assert_eq!(ttl, -2);

        let fetched: Option<TestData> = store.get(&session_id, field).await.unwrap();
        assert!(fetched.is_none());

        store.delete(&session_id).await.unwrap();
        let all = store.get_all(&session_id).await.unwrap();
        assert!(all.is_none());
    }

    // --- Rename ---
    #[tokio::test]
    async fn test_insert_with_rename() {
        let store = setup_store().await;
        let old_id = Id::default();
        let new_id = Id::default();
        let field = "rename_field";
        let value = TestData { value: "rename".into() };

        store.insert_with_rename(&old_id, &new_id, field, &value, Some(60), Some(60))
            .await.unwrap();

        assert!(store.get::<TestData>(&old_id, field).await.unwrap().is_none());
        assert_eq!(store.get::<TestData>(&new_id, field).await.unwrap(), Some(value));
    }

    #[tokio::test]
    async fn test_rename_to_existing_session() {
        let store = setup_store().await;
        let old_id = Id::default();
        let new_id = Id::default();
        let field_old = "f1";
        let field_new = "f2";

        store.insert(&old_id, field_old, &TestData { value: "v1".into() }, Some(60), Some(60))
            .await.unwrap();
        store.insert(&new_id, field_new, &TestData { value: "v2".into() }, Some(60), Some(60))
            .await.unwrap();

        store.update_with_rename(&old_id, &new_id, field_old, &TestData { value: "v1_upd".into() }, Some(60), Some(60))
            .await.unwrap();

        assert!(store.get::<TestData>(&old_id, field_old).await.unwrap().is_none());
        let fetched_new: Option<TestData> = store.get(&new_id, field_old).await.unwrap();
        assert_eq!(fetched_new.unwrap().value, "v1_upd");
    }

    // --- Get All ---
    #[tokio::test]
    async fn test_get_all_multiple_fields() {
        let store = setup_store().await;
        let session_id = Id::default();

        store.insert(&session_id, "f1", &TestData { value: "v1".into() }, Some(60), Some(60)).await.unwrap();
        store.insert(&session_id, "f2", &TestData { value: "v2".into() }, Some(60), Some(60)).await.unwrap();

        let all = store.get_all(&session_id).await.unwrap().unwrap();
        assert_eq!(all.len(), 2);
    }
}
