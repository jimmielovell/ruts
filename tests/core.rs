mod common;

#[cfg(test)]
mod tests {
    use super::*;

    use common::*;
    use ruts::store::MemoryStore;
    use ruts::store::SessionStore;
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
