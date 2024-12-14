#[cfg(test)]
mod tests {
    use fred::clients::Client;
    use fred::interfaces::ClientLike;
    use mock::MockSessionStore;
    use ruts::store::redis::RedisStore;
    use ruts::store::SessionStore;
    use ruts::{Inner, Session};
    use serde::{Deserialize, Serialize};
    use std::sync::Arc;

    // Test data structures
    #[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
    struct TestUser {
        id: i64,
        name: String,
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
    struct TestSession {
        user: TestUser,
        preferences: TestPreferences,
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
    struct TestPreferences {
        theme: String,
        language: String,
    }

    // Mock session store for testing
    mod mock {
        use super::*;
        use ruts::store::Error;
        use ruts::Id;
        use serde::de::DeserializeOwned;
        use std::collections::HashMap;
        use std::sync::Arc;
        use tokio::sync::RwLock;

        #[derive(Clone, Debug)]
        pub struct MockSessionStore {
            data: Arc<RwLock<HashMap<String, HashMap<String, Vec<u8>>>>>,
        }

        impl MockSessionStore {
            pub fn new() -> Self {
                Self {
                    data: Arc::new(RwLock::new(HashMap::new())),
                }
            }
        }

        #[async_trait::async_trait]
        impl SessionStore for MockSessionStore {
            async fn get<T>(&self, session_id: &Id, field: &str) -> Result<Option<T>, Error>
            where
                T: Clone + Send + Sync + DeserializeOwned,
            {
                let data = self.data.read().await;
                if let Some(session) = data.get(&session_id.to_string()) {
                    if let Some(value) = session.get(field) {
                        return Ok(Some(
                            rmp_serde::from_slice(value)
                                .map_err(|e| Error::Decode(e.to_string()))?,
                        ));
                    }
                }
                Ok(None)
            }

            async fn get_all<T>(&self, session_id: &Id) -> Result<Option<T>, Error>
            where
                T: Clone + Send + Sync + DeserializeOwned,
            {
                let data = self.data.read().await;
                if let Some(session) = data.get(&session_id.to_string()) {
                    if let Some(value) = session.get("__all") {
                        return Ok(Some(
                            rmp_serde::from_slice(value)
                                .map_err(|e| Error::Decode(e.to_string()))?,
                        ));
                    }
                }
                Ok(None)
            }

            async fn insert<T>(
                &self,
                session_id: &Id,
                field: &str,
                value: &T,
                _seconds: i64,
            ) -> Result<bool, Error>
            where
                T: Send + Sync + Serialize,
            {
                let mut data = self.data.write().await;
                let session = data
                    .entry(session_id.to_string())
                    .or_insert_with(HashMap::new);
                if session.contains_key(field) {
                    return Ok(false);
                }
                let encoded = rmp_serde::to_vec(value).map_err(|e| Error::Encode(e.to_string()))?;
                session.insert(field.to_string(), encoded);
                Ok(true)
            }

            async fn update<T>(
                &self,
                session_id: &Id,
                field: &str,
                value: &T,
                _seconds: i64,
            ) -> Result<bool, Error>
            where
                T: Send + Sync + Serialize,
            {
                let mut data = self.data.write().await;
                let session = data
                    .entry(session_id.to_string())
                    .or_insert_with(HashMap::new);
                let encoded = rmp_serde::to_vec(value).map_err(|e| Error::Encode(e.to_string()))?;
                session.insert(field.to_string(), encoded);
                Ok(true)
            }

            async fn rename_session_id(
                &self,
                old_session_id: &Id,
                new_session_id: &Id,
                _seconds: i64,
            ) -> Result<bool, Error> {
                let mut data = self.data.write().await;
                if let Some(session) = data.remove(&old_session_id.to_string()) {
                    data.insert(new_session_id.to_string(), session);
                    Ok(true)
                } else {
                    Ok(false)
                }
            }

            async fn remove(&self, session_id: &Id, field: &str) -> Result<i8, Error> {
                let mut data = self.data.write().await;
                if let Some(session) = data.get_mut(&session_id.to_string()) {
                    session.remove(field);
                    Ok(if session.is_empty() { 0 } else { 1 })
                } else {
                    Ok(0)
                }
            }

            async fn delete(&self, session_id: &Id) -> Result<bool, Error> {
                let mut data = self.data.write().await;
                Ok(data.remove(&session_id.to_string()).is_some())
            }

            async fn expire(&self, _session_id: &Id, _seconds: i64) -> Result<bool, Error> {
                Ok(true)
            }
        }
    }

    fn create_test_session() -> TestSession {
        TestSession {
            user: TestUser {
                id: 1,
                name: "Test User".to_string(),
            },
            preferences: TestPreferences {
                theme: "dark".to_string(),
                language: "en".to_string(),
            },
        }
    }

    fn create_inner<S: SessionStore>(
        store: Arc<S>,
        cookie_name: Option<&'static str>,
        cookie_max_age: Option<i64>,
    ) -> Arc<Inner<S>> {
        Arc::new(Inner::new(store, cookie_name, cookie_max_age))
    }

    async fn setup_redis() -> Arc<RedisStore<Client>> {
        let client = Client::default();
        client.connect();
        client.wait_for_connect().await.unwrap();
        Arc::new(RedisStore::new(Arc::new(client)))
    }

    // Tests
    #[tokio::test]
    async fn test_redis_integration() {
        let store = setup_redis().await;
        let inner = create_inner(store, Some("test_sess"), Some(3600));
        let session = Session::new(inner);
        let test_data = create_test_session();

        // Test Redis operations
        session.insert("test", &test_data).await.unwrap();
        let retrieved: Option<TestSession> = session.get("test").await.unwrap();
        assert_eq!(retrieved.unwrap(), test_data);

        session.delete().await.unwrap();
        let retrieved: Option<TestSession> = session.get("test").await.unwrap();
        assert!(retrieved.is_none());
    }
    
    #[tokio::test]
    async fn test_session_regenerate() {
        let store = Arc::new(MockSessionStore::new());
        let inner = create_inner(store, Some("test_sess"), Some(3600));
        let session = Session::new(inner);
        let test_data = create_test_session();

        // Insert initial data
        session.insert("test", &test_data).await.unwrap();
        let original_id = session.id().unwrap();

        // Regenerate session
        let new_id = session.regenerate().await.unwrap();
        assert!(new_id.is_some());
        assert_ne!(original_id, new_id.unwrap());

        // Verify data persistence
        let retrieved: Option<TestSession> = session.get("test").await.unwrap();
        assert_eq!(retrieved.unwrap(), test_data);
    }

    #[tokio::test]
    async fn test_session_state_management() {
        let store = Arc::new(MockSessionStore::new());
        let inner = create_inner(store, Some("test_sess"), Some(3600));
        let session = Session::new(inner);
        let test_data = create_test_session();

        // Test initial state - can check through behavior
        let initial_get: Option<TestSession> = session.get("test").await.unwrap();
        assert!(initial_get.is_none());

        // Insert data and verify
        session.insert("test", &test_data).await.unwrap();
        let after_insert: Option<TestSession> = session.get("test").await.unwrap();
        assert!(after_insert.is_some());

        // Test delete behavior
        session.delete().await.unwrap();
        let after_delete: Option<TestSession> = session.get("test").await.unwrap();
        assert!(after_delete.is_none());
    }
}
