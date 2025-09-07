mod common;

#[cfg(test)]
mod tests {
    use super::*;

    use common::*;
    use ruts::store::SessionStore;
    use ruts::store::memory::MemoryStore;
    use ruts::{Inner, Session};
    use std::sync::Arc;

    fn create_inner<S: SessionStore>(
        store: Arc<S>,
        cookie_name: Option<&'static str>,
        cookie_max_age: Option<i64>,
    ) -> Arc<Inner<S>> {
        Arc::new(Inner::new(store, cookie_name, cookie_max_age))
    }

    #[tokio::test]
    async fn test_session_basic_operations() {
        let store = Arc::new(MemoryStore::new());
        let inner = create_inner(store, Some("test_sess"), Some(3600));
        let session = Session::new(inner);
        let test_data = create_test_session();

        // Test initial state
        let initial_get: Option<TestSession> = session.get("test").await.unwrap();
        assert!(initial_get.is_none());

        // Test insert
        let inserted = session.insert("test", &test_data, None).await.unwrap();
        assert!(inserted);

        // Test get after insert
        let retrieved: Option<TestSession> = session.get("test").await.unwrap();
        assert_eq!(retrieved.unwrap(), test_data);

        // Test update
        let mut updated_data = test_data.clone();
        updated_data.user.name = "Updated User".to_string();
        let updated = session.update("test", &updated_data, None).await.unwrap();
        assert!(updated);

        // Verify update
        let retrieved: Option<TestSession> = session.get("test").await.unwrap();
        assert_eq!(retrieved.unwrap(), updated_data);

        // Test delete
        let deleted = session.delete().await.unwrap();
        assert!(deleted);

        // Verify deletion
        let retrieved: Option<TestSession> = session.get("test").await.unwrap();
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_prepare_regenerate_with_update() {
        let store = Arc::new(MemoryStore::new());
        let inner = create_inner(store.clone(), Some("test_sess"), Some(3600));
        let session = Session::new(inner);
        let test_data = create_test_session();

        // Insert initial data
        session.insert("test", &test_data, None).await.unwrap();
        let original_id = session.id().unwrap();

        // Prepare new ID and update
        let prepared_id = session.prepare_regenerate();
        let mut updated_data = test_data.clone();
        updated_data.user.name = "Updated User".to_string();
        let updated = session.update("test", &updated_data, None).await.unwrap();
        assert!(updated);

        // Verify ID changed and data updated
        let current_id = session.id().unwrap();
        assert_eq!(current_id, prepared_id);
        assert_ne!(current_id, original_id);

        let retrieved: Option<TestSession> = session.get("test").await.unwrap();
        assert_eq!(retrieved.unwrap(), updated_data);

        // Verify old session is gone by directly checking the store
        let result: Result<Option<TestSession>, _> = store.get(&original_id, "test").await;
        assert!(result.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_prepare_regenerate_with_insert() {
        let store = Arc::new(MemoryStore::new());
        let inner = create_inner(store.clone(), Some("test_sess"), Some(3600));
        let session = Session::new(inner);
        let test_data = create_test_session();

        // Insert initial data
        session.insert("test1", &test_data, None).await.unwrap();
        let original_id = session.id().unwrap();

        // Prepare new ID and insert new field
        let prepared_id = session.prepare_regenerate();
        let mut new_data = test_data.clone();
        new_data.user.name = "New User".to_string();
        let inserted = session.insert("test2", &new_data, None).await.unwrap();
        assert!(inserted);

        // Verify ID changed and both fields exist
        let current_id = session.id().unwrap();
        assert_eq!(current_id, prepared_id);
        assert_ne!(current_id, original_id);

        let retrieved1: Option<TestSession> = session.get("test1").await.unwrap();
        let retrieved2: Option<TestSession> = session.get("test2").await.unwrap();
        assert_eq!(retrieved1.unwrap(), test_data);
        assert_eq!(retrieved2.unwrap(), new_data);

        // Verify old session is gone by directly checking the store
        let result: Result<Option<TestSession>, _> = store.get(&original_id, "test1").await;
        assert!(result.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_multiple_prepare_regenerate() {
        let store = Arc::new(MemoryStore::new());
        let inner = create_inner(store.clone(), Some("test_sess"), Some(3600));
        let session = Session::new(inner);
        let test_data = create_test_session();

        // Insert initial data
        session.insert("test", &test_data, None).await.unwrap();
        let original_id = session.id().unwrap();

        // First prepare_regenerate
        let first_prepared_id = session.prepare_regenerate();

        // Second prepare_regenerate before any operation
        let second_prepared_id = session.prepare_regenerate();
        assert_ne!(first_prepared_id, second_prepared_id);

        // Update - should use the last prepared ID
        let mut updated_data = test_data.clone();
        updated_data.user.name = "Updated User".to_string();
        session.update("test", &updated_data, None).await.unwrap();

        // Verify the last prepared ID was used
        let current_id = session.id().unwrap();
        assert_eq!(current_id, second_prepared_id);
        assert_ne!(current_id, first_prepared_id);
        assert_ne!(current_id, original_id);

        // Verify original session is gone by directly checking the store
        let result: Result<Option<TestSession>, _> = store.get(&original_id, "test").await;
        assert!(result.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_session_regeneration() {
        let store = Arc::new(MemoryStore::new());
        let inner = create_inner(store, Some("test_sess"), Some(3600));
        let session = Session::new(inner);
        let test_data = create_test_session();

        // Insert initial data
        session.insert("test", &test_data, None).await.unwrap();
        let original_id = session.id().unwrap();

        // Regenerate session
        let new_id = session.regenerate().await.unwrap();
        assert!(new_id.is_some());
        assert_ne!(original_id, new_id.unwrap());

        // Verify data persistence
        let retrieved: Option<TestSession> = session.get("test").await.unwrap();
        assert_eq!(retrieved.unwrap(), test_data);
    }
}
