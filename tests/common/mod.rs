use ruts::store::{Error, SessionStore};
use ruts::{Id, Session};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct TestData {
    pub f1: i64,
    pub f2: String,
}

pub fn create_test_data() -> TestData {
    TestData {
        f1: 1,
        f2: "Test".to_string(),
    }
}

pub async fn helper_set<S: SessionStore, T: Serialize + Send + Sync>(
    store: &S,
    session_id: &Id,
    field: &str,
    value: &T,
    key_ttl: i64,
    field_ttl: i64,
    hot_ttl: Option<i64>,
) -> Result<i64, Error> {
    #[cfg(feature = "layered-store")]
    return store
        .set(session_id, field, value, key_ttl, field_ttl, hot_ttl)
        .await;
    #[cfg(not(feature = "layered-store"))]
    return store
        .set(session_id, field, value, key_ttl, field_ttl, None)
        .await;
}

pub async fn helper_set_and_rename<S: SessionStore, T: Serialize + Send + Sync>(
    store: &S,
    old_id: &Id,
    new_id: &Id,
    field: &str,
    value: &T,
    key_ttl: i64,
    field_ttl: i64,
    hot_ttl: Option<i64>,
) -> Result<i64, Error> {
    #[cfg(feature = "layered-store")]
    return store
        .set_and_rename(old_id, new_id, field, value, key_ttl, field_ttl, hot_ttl)
        .await;
    #[cfg(not(feature = "layered-store"))]
    return store
        .set_and_rename(old_id, new_id, field, value, key_ttl, field_ttl, None)
        .await;
}

pub async fn run_basic_crud<S: SessionStore>(store: &S) {
    let session_id = Id::default();
    let field = "field1";
    let data = create_test_data();

    // 1. Create & assert TTL return
    let ttl = helper_set(store, &session_id, field, &data, 60, 60, None)
        .await
        .unwrap();
    assert!(ttl > 55, "Expected TTL > 55, got {}", ttl);

    // 2. Read
    let fetched: Option<TestData> = store.get(&session_id, field).await.unwrap();
    assert_eq!(fetched, Some(data.clone()));

    // 3. Update & Overwrite
    let updated_data = TestData {
        f1: 2,
        f2: "world".into(),
    };
    helper_set(store, &session_id, field, &updated_data, 60, 60, None)
        .await
        .unwrap();
    let fetched: Option<TestData> = store.get(&session_id, field).await.unwrap();
    assert_eq!(fetched, Some(updated_data));

    // 4. Remove Single Field
    let remove_ttl = store.remove(&session_id, field).await.unwrap();
    assert_eq!(remove_ttl, -2); // Should be -2 since it was the only field

    let fetched: Option<TestData> = store.get(&session_id, field).await.unwrap();
    assert!(fetched.is_none());

    // 5. Delete Entire Session
    helper_set(
        store,
        &session_id,
        "to_remove",
        &TestData {
            f1: 3,
            f2: "bye".into(),
        },
        60,
        60,
        None,
    )
    .await
    .unwrap();

    store.delete(&session_id).await.unwrap();

    let all = store.get_all(&session_id).await.unwrap();
    assert!(all.is_none());
}

pub async fn run_ttl_zero_removes<S: SessionStore>(store: &S) {
    let session_id = Id::default();
    let field = "ttl_zero";

    let ttl = helper_set(
        store,
        &session_id,
        field,
        &TestData {
            f1: 4,
            f2: "x".into(),
        },
        60,
        0, // Passing 0 should trigger removal
        None,
    )
    .await
    .unwrap();
    assert_eq!(ttl, -2);

    let fetched: Option<TestData> = store.get(&session_id, field).await.unwrap();
    assert!(fetched.is_none());
}

pub async fn run_get_all<S: SessionStore>(store: &S) {
    let session_id = Id::default();

    let data1 = TestData {
        f1: 5,
        f2: "1".into(),
    };
    let data2 = TestData {
        f1: 6,
        f2: "2".into(),
    };

    helper_set(store, &session_id, "f1", &data1, 60, 60, None)
        .await
        .unwrap();
    helper_set(store, &session_id, "f2", &data2, 60, 60, None)
        .await
        .unwrap();

    let map = store.get_all(&session_id).await.unwrap().unwrap();
    assert_eq!(map.len(), 2);

    let v1: Option<TestData> = map.get("f1").unwrap();
    let v2: Option<TestData> = map.get("f2").unwrap();
    assert_eq!(v1, Some(data1));
    assert_eq!(v2, Some(data2));
}

pub async fn run_rename<S: SessionStore>(store: &S) {
    let old_id = Id::default();
    let new_id = Id::default();
    let field = "f1";
    let data = TestData {
        f1: 7,
        f2: "rename_me".into(),
    };

    helper_set(store, &old_id, field, &data, 60, 60, None)
        .await
        .unwrap();

    let renamed = store.rename_session_id(&old_id, &new_id).await.unwrap();
    assert!(renamed);

    let new_fetch: Option<TestData> = store.get(&new_id, field).await.unwrap();
    assert_eq!(new_fetch, Some(data.clone()));

    // Set and Rename handling a completely non-existent old session ID
    let ghost_id = Id::default();
    let newest_id = Id::default();
    let new_data = TestData {
        f1: 8,
        f2: "new".into(),
    };

    helper_set_and_rename(store, &ghost_id, &newest_id, "f2", &new_data, 60, 60, None)
        .await
        .unwrap();

    let ghost_fetch: Option<TestData> = store.get(&ghost_id, "f2").await.unwrap();
    let newest_fetch: Option<TestData> = store.get(&newest_id, "f2").await.unwrap();

    assert!(ghost_fetch.is_none());
    assert_eq!(newest_fetch, Some(new_data));
}

pub async fn run_expiry<S: SessionStore>(store: &S) {
    let session_id = Id::default();
    let field = "f1";
    let data = TestData {
        f1: 9,
        f2: "x".into(),
    };

    helper_set(store, &session_id, field, &data, 60, 60, None)
        .await
        .unwrap();

    // Expire immediately (0)
    store.expire(&session_id, 0).await.unwrap();

    let fetch: Option<TestData> = store.get(&session_id, field).await.unwrap();
    assert!(fetch.is_none());
}

#[cfg(feature = "layered-store")]
pub async fn run_layered_hot<S: ruts::store::LayeredHotStore + SessionStore>(store: &S) {
    let session_id = Id::default();
    let data1 = ruts::store::serialize_value(&TestData {
        f1: 10,
        f2: "1".into(),
    })
    .unwrap();
    let data2 = ruts::store::serialize_value(&TestData {
        f1: 11,
        f2: "2".into(),
    })
    .unwrap();

    let pairs: Vec<(&str, &[u8], Option<i64>)> =
        vec![("hot1", &data1, Some(60)), ("hot2", &data2, Some(120))];

    store.set_multiple(&session_id, &pairs).await.unwrap();

    let v1: Option<TestData> = store.get(&session_id, "hot1").await.unwrap();
    let v2: Option<TestData> = store.get(&session_id, "hot2").await.unwrap();

    assert_eq!(
        v1,
        Some(TestData {
            f1: 10,
            f2: "1".into()
        })
    );
    assert_eq!(
        v2,
        Some(TestData {
            f1: 11,
            f2: "2".into()
        })
    );
}

#[cfg(feature = "layered-store")]
pub async fn run_layered_cold<S: ruts::store::LayeredColdStore + SessionStore>(store: &S) {
    let session_id = Id::default();

    store
        .set_with_meta(
            &session_id,
            "cold1",
            &TestData {
                f1: 12,
                f2: "1".into(),
            },
            60,
            60,
            Some(30),
        )
        .await
        .unwrap();

    store
        .set_with_meta(
            &session_id,
            "cold2",
            &TestData {
                f1: 13,
                f2: "2".into(),
            },
            60,
            60,
            None,
        )
        .await
        .unwrap();

    let (session_map, meta_map) = store.get_all_with_meta(&session_id).await.unwrap().unwrap();

    assert_eq!(session_map.len(), 2);
    assert_eq!(meta_map.get("cold1"), Some(&Some(30)));
    assert_eq!(meta_map.get("cold2"), Some(&None));
}

pub async fn run_session_operations<S: SessionStore>(session: Session<S>) {
    let test_data = create_test_data();

    let inserted = session.set("test", &test_data, None, None).await.unwrap();
    assert!(inserted);

    let retrieved: Option<TestData> = session.get("test").await.unwrap();
    assert_eq!(retrieved.unwrap(), test_data);

    let mut new_data = test_data.clone();
    new_data.f2 = "New Name".to_string();

    let inserted_again = session.set("test", &new_data, None, None).await.unwrap();
    assert!(inserted_again, "Insert should succeed (overwrite)");

    let retrieved_new: Option<TestData> = session.get("test").await.unwrap();
    assert_eq!(retrieved_new.unwrap(), new_data);

    let deleted = session.delete().await.unwrap();
    assert!(deleted);

    let retrieved: Option<TestData> = session.get("test").await.unwrap();
    assert!(retrieved.is_none());
}

pub async fn run_session_prepare_regenerate<S: SessionStore>(store: &S, session: Session<S>) {
    let test_data = create_test_data();

    session.set("test1", &test_data, None, None).await.unwrap();
    let original_id = session.id().unwrap();

    let prepared_id = session.prepare_regenerate();
    let mut new_data = test_data.clone();
    new_data.f2 = "New User".to_string();

    // This update should trigger the rename of the session AND set the new field
    let inserted = session.set("test2", &new_data, None, None).await.unwrap();
    assert!(inserted);

    // Verify id changed and both fields exist on the NEW id
    let current_id = session.id().unwrap();
    assert_eq!(current_id.to_string(), prepared_id.to_string());
    assert_ne!(current_id.to_string(), original_id.to_string());

    let retrieved1: Option<TestData> = session.get("test1").await.unwrap();
    let retrieved2: Option<TestData> = session.get("test2").await.unwrap();
    assert_eq!(retrieved1.unwrap(), test_data);
    assert_eq!(retrieved2.unwrap(), new_data);

    // Verify old session is gone
    let result: Option<TestData> = store.get(&original_id, "test1").await.unwrap();
    assert!(result.is_none());
}

/// Stamps out the standard `SessionStore` tests for any backend.
#[macro_export]
macro_rules! define_session_store_tests {
    ($setup:ident) => {
        #[tokio::test]
        async fn test_store_basic_crud() {
            let store = $setup().await;
            common::run_basic_crud(&*store).await;
        }
        #[tokio::test]
        async fn test_store_ttl_zero_removes() {
            let store = $setup().await;
            common::run_ttl_zero_removes(&*store).await;
        }
        #[tokio::test]
        async fn test_store_get_all() {
            let store = $setup().await;
            common::run_get_all(&*store).await;
        }
        #[tokio::test]
        async fn test_store_rename() {
            let store = $setup().await;
            common::run_rename(&*store).await;
        }
        #[tokio::test]
        async fn test_store_expiry() {
            let store = $setup().await;
            common::run_expiry(&*store).await;
        }
    };
}

/// Stamps out the `LayeredHotStore` tests for compatible backends.
#[macro_export]
macro_rules! define_layered_hot_store_tests {
    ($setup:ident) => {
        #[cfg(feature = "layered-store")]
        #[tokio::test]
        async fn test_hot_store_multiple() {
            let store = $setup().await;
            common::run_layered_hot(&*store).await;
        }
    };
}

/// Stamps out the `LayeredColdStore` tests for compatible backends.
#[macro_export]
macro_rules! define_layered_cold_store_tests {
    ($setup:ident) => {
        #[cfg(feature = "layered-store")]
        #[tokio::test]
        async fn test_cold_store_meta() {
            let store = $setup().await;
            common::run_layered_cold(&*store).await;
        }
    };
}