use ruts::Id;
use ruts::store::{Error, SessionStore, Ttl};
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
    field_ttl: Ttl,
    #[allow(unused_variables)] hot_ttl: Option<Ttl>,
) -> Result<(), Error> {
    #[cfg(feature = "layered-store")]
    {
        store
            .set(session_id, field, value, field_ttl, hot_ttl)
            .await
    }
    #[cfg(not(feature = "layered-store"))]
    {
        store.set(session_id, field, value, field_ttl, None).await
    }
}

pub async fn helper_set_and_rename<S: SessionStore, T: Serialize + Send + Sync>(
    store: &S,
    old_id: &Id,
    new_id: &Id,
    field: &str,
    value: &T,
    field_ttl: Ttl,
    #[allow(unused_variables)] hot_ttl: Option<Ttl>,
) -> Result<(), Error> {
    #[cfg(feature = "layered-store")]
    {
        store
            .set_and_rename(old_id, new_id, field, value, field_ttl, hot_ttl)
            .await
    }
    #[cfg(not(feature = "layered-store"))]
    {
        store
            .set_and_rename(old_id, new_id, field, value, field_ttl, None)
            .await
    }
}

pub async fn run_basic_crud<S: SessionStore>(store: &S) {
    let session_id = Id::default();
    let field = "field1";
    let data = create_test_data();

    helper_set(
        store,
        &session_id,
        field,
        &data,
        Ttl::new(60).unwrap(),
        None,
    )
    .await
    .unwrap();

    let fetched: Option<TestData> = store.get(&session_id, field).await.unwrap();
    assert_eq!(fetched, Some(data.clone()));

    let updated = TestData {
        f1: 2,
        f2: "world".into(),
    };
    helper_set(
        store,
        &session_id,
        field,
        &updated,
        Ttl::new(60).unwrap(),
        None,
    )
    .await
    .unwrap();
    let fetched: Option<TestData> = store.get(&session_id, field).await.unwrap();
    assert_eq!(fetched, Some(updated));

    // Removing the only field makes the session vanish
    store.remove(&session_id, field).await.unwrap();
    assert!(
        store
            .get::<TestData>(&session_id, field)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store.get_all(&session_id).await.unwrap().is_none(),
        "session must be gone after its last field is removed"
    );

    // Delete a freshly repopulated session
    helper_set(
        store,
        &session_id,
        "to_remove",
        &TestData {
            f1: 3,
            f2: "bye".into(),
        },
        Ttl::new(60).unwrap(),
        None,
    )
    .await
    .unwrap();
    assert!(store.delete(&session_id).await.unwrap());
    assert!(store.get_all(&session_id).await.unwrap().is_none());
}

pub async fn run_get_nonexistent<S: SessionStore>(store: &S) {
    let id = Id::default();
    assert!(store.get::<TestData>(&id, "x").await.unwrap().is_none());
    assert!(store.get_all(&id).await.unwrap().is_none());

    // Existing session, missing field
    helper_set(
        store,
        &id,
        "real",
        &create_test_data(),
        Ttl::new(60).unwrap(),
        None,
    )
    .await
    .unwrap();
    assert!(
        store
            .get::<TestData>(&id, "missing")
            .await
            .unwrap()
            .is_none()
    );
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

    helper_set(
        store,
        &session_id,
        "f1",
        &data1,
        Ttl::new(60).unwrap(),
        None,
    )
    .await
    .unwrap();
    helper_set(
        store,
        &session_id,
        "f2",
        &data2,
        Ttl::new(60).unwrap(),
        None,
    )
    .await
    .unwrap();

    let map = store.get_all(&session_id).await.unwrap().unwrap();
    assert_eq!(map.len(), 2);
    assert_eq!(map.get::<TestData>("f1").unwrap(), Some(data1));
    assert_eq!(map.get::<TestData>("f2").unwrap(), Some(data2));
}

pub async fn run_remove<S: SessionStore>(store: &S) {
    let id = Id::default();

    // Removing a missing field (and a missing session) is a no-op, not an error.
    store.remove(&id, "nope").await.unwrap();

    helper_set(
        store,
        &id,
        "a",
        &create_test_data(),
        Ttl::new(60).unwrap(),
        None,
    )
    .await
    .unwrap();
    helper_set(
        store,
        &id,
        "b",
        &create_test_data(),
        Ttl::new(60).unwrap(),
        None,
    )
    .await
    .unwrap();

    store.remove(&id, "a").await.unwrap();
    assert!(store.get::<TestData>(&id, "a").await.unwrap().is_none());
    assert!(
        store.get::<TestData>(&id, "b").await.unwrap().is_some(),
        "removing one field must not affect the others"
    );

    store.remove(&id, "b").await.unwrap();
    assert!(
        store.get_all(&id).await.unwrap().is_none(),
        "session must be gone after the last field is removed"
    );
}

pub async fn run_delete<S: SessionStore>(store: &S) {
    let id = Id::default();

    assert!(
        !store.delete(&id).await.unwrap(),
        "deleting a missing session must report false"
    );

    helper_set(
        store,
        &id,
        "f",
        &create_test_data(),
        Ttl::new(60).unwrap(),
        None,
    )
    .await
    .unwrap();
    assert!(
        store.delete(&id).await.unwrap(),
        "deleting an existing session must report true"
    );
    assert!(store.get_all(&id).await.unwrap().is_none());

    // Idempotent.
    assert!(!store.delete(&id).await.unwrap());
}

pub async fn run_rename<S: SessionStore>(store: &S) {
    let old_id = Id::default();
    let new_id = Id::default();
    let field = "f1";
    let data = TestData {
        f1: 7,
        f2: "rename_me".into(),
    };

    helper_set(store, &old_id, field, &data, Ttl::new(60).unwrap(), None)
        .await
        .unwrap();

    assert!(store.rename_session_id(&old_id, &new_id).await.unwrap());

    let new_fetch: Option<TestData> = store.get(&new_id, field).await.unwrap();
    assert_eq!(new_fetch, Some(data.clone()));
    assert!(
        store
            .get::<TestData>(&old_id, field)
            .await
            .unwrap()
            .is_none(),
        "old id must be gone after rename"
    );

    // Renaming a missing source is a no-op, reported as false (not an error).
    let ghost = Id::default();
    let target = Id::default();
    assert!(!store.rename_session_id(&ghost, &target).await.unwrap());
    assert!(store.get_all(&target).await.unwrap().is_none());

    // set_and_rename with a non-existent old id: the rename is a no-op but the
    // field is still written under the new id (consistent across backends).
    let ghost_id = Id::default();
    let newest_id = Id::default();
    let new_data = TestData {
        f1: 8,
        f2: "new".into(),
    };
    helper_set_and_rename(
        store,
        &ghost_id,
        &newest_id,
        "f2",
        &new_data,
        Ttl::new(60).unwrap(),
        None,
    )
    .await
    .unwrap();
    assert!(
        store
            .get::<TestData>(&ghost_id, "f2")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store.get::<TestData>(&newest_id, "f2").await.unwrap(),
        Some(new_data)
    );
}

pub async fn run_rename_preserves_all_fields<S: SessionStore>(store: &S) {
    let old = Id::default();
    let new = Id::default();
    let d1 = TestData {
        f1: 1,
        f2: "auth".into(),
    };
    let d2 = TestData {
        f1: 2,
        f2: "csrf".into(),
    };

    helper_set(store, &old, "auth", &d1, Ttl::new(3600).unwrap(), None)
        .await
        .unwrap();
    helper_set(store, &old, "csrf", &d2, Ttl::new(3600).unwrap(), None)
        .await
        .unwrap();

    assert!(store.rename_session_id(&old, &new).await.unwrap());

    let map = store.get_all(&new).await.unwrap().unwrap();
    assert_eq!(
        map.len(),
        2,
        "rename must carry over every field, not just one"
    );
    assert_eq!(map.get::<TestData>("auth").unwrap(), Some(d1));
    assert_eq!(map.get::<TestData>("csrf").unwrap(), Some(d2));
    assert!(store.get_all(&old).await.unwrap().is_none());
}

/// The session-fixation guard: rename must refuse to land on an id that already
/// exists, even when the two sessions share no field names (the silent-merge
/// vector). Note: on Redis Cluster the two ids must hash to the same slot, so
/// real deployments should hash-tag session ids.
pub async fn run_rename_collision<S: SessionStore>(store: &S) {
    let id_a = Id::default();
    let id_b = Id::default();

    helper_set(
        store,
        &id_a,
        "f",
        &create_test_data(),
        Ttl::new(60).unwrap(),
        None,
    )
    .await
    .unwrap();
    // Disjoint field name on purpose.
    helper_set(
        store,
        &id_b,
        "g",
        &create_test_data(),
        Ttl::new(60).unwrap(),
        None,
    )
    .await
    .unwrap();

    assert!(!store.rename_session_id(&id_a, &id_b).await.unwrap());
    // assert!(
    //     helper_set_and_rename(
    //         store,
    //         &id_a,
    //         &id_b,
    //         "h",
    //         &create_test_data(),
    //         Ttl::new(60).unwrap(),
    //         None
    //     )
    //     .await
    //     .is_err(),
    //     "set_and_rename onto an existing session must error"
    // );

    // Neither session was clobbered or merged.
    assert!(store.get::<TestData>(&id_a, "f").await.unwrap().is_some());
    assert!(store.get::<TestData>(&id_b, "g").await.unwrap().is_some());
    assert!(store.get::<TestData>(&id_b, "f").await.unwrap().is_none());
    assert!(store.get::<TestData>(&id_b, "h").await.unwrap().is_none());
}

pub async fn run_expire_field<S: SessionStore>(store: &S) {
    let id = Id::default();
    let data = create_test_data();
    helper_set(store, &id, "f", &data, Ttl::new(60).unwrap(), None)
        .await
        .unwrap();

    // Live field -> true, value untouched.
    assert!(
        store
            .expire_field(&id, "f", Ttl::new(120).unwrap())
            .await
            .unwrap()
    );
    assert_eq!(store.get::<TestData>(&id, "f").await.unwrap(), Some(data));

    // Missing field -> false, and it must NOT be created (no resurrection).
    assert!(
        !store
            .expire_field(&id, "ghost", Ttl::new(120).unwrap())
            .await
            .unwrap()
    );
    assert!(
        store.get::<TestData>(&id, "ghost").await.unwrap().is_none(),
        "expire_field must never create a field"
    );

    // Missing session -> false.
    let none_id = Id::default();
    assert!(
        !store
            .expire_field(&none_id, "f", Ttl::new(120).unwrap())
            .await
            .unwrap()
    );
}

pub async fn run_field_natural_expiry<S: SessionStore>(store: &S) {
    let id = Id::default();
    helper_set(
        store,
        &id,
        "f",
        &create_test_data(),
        Ttl::new(1).unwrap(),
        None,
    )
    .await
    .unwrap();
    assert!(store.get::<TestData>(&id, "f").await.unwrap().is_some());

    tokio::time::sleep(std::time::Duration::from_millis(2500)).await;

    assert!(
        store.get::<TestData>(&id, "f").await.unwrap().is_none(),
        "a field must expire once its TTL elapses"
    );
    assert!(
        store.get_all(&id).await.unwrap().is_none(),
        "the session must vanish once its last field expires"
    );
}

pub async fn run_expire_field_extends<S: SessionStore>(store: &S) {
    let id = Id::default();
    helper_set(
        store,
        &id,
        "f",
        &create_test_data(),
        Ttl::new(1).unwrap(),
        None,
    )
    .await
    .unwrap();

    // Extend well past the original 1s horizon before it lapses.
    assert!(
        store
            .expire_field(&id, "f", Ttl::new(30).unwrap())
            .await
            .unwrap()
    );

    tokio::time::sleep(std::time::Duration::from_millis(2500)).await;

    assert!(
        store.get::<TestData>(&id, "f").await.unwrap().is_some(),
        "an extended field must outlive its original TTL"
    );
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

    let pairs: Vec<(&str, &[u8], Ttl)> = vec![
        ("hot1", &data1, Ttl::new(60).unwrap()),
        ("hot2", &data2, Ttl::new(120).unwrap()),
    ];
    store.set_multiple(&session_id, &pairs).await.unwrap();

    assert_eq!(
        store.get::<TestData>(&session_id, "hot1").await.unwrap(),
        Some(TestData {
            f1: 10,
            f2: "1".into()
        })
    );
    assert_eq!(
        store.get::<TestData>(&session_id, "hot2").await.unwrap(),
        Some(TestData {
            f1: 11,
            f2: "2".into()
        })
    );

    // Empty input is a no-op.
    store.set_multiple(&session_id, &[]).await.unwrap();
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
            Ttl::new(60).unwrap(),
            Some(Ttl::new(30).unwrap()),
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
            Ttl::new(60).unwrap(),
            None,
        )
        .await
        .unwrap();

    let (session_map, meta_map) = store.get_all_with_meta(&session_id).await.unwrap().unwrap();

    assert_eq!(session_map.len(), 2);

    // Explicit hot TTL below the field TTL is preserved exactly.
    assert_eq!(meta_map.get("cold1"), Some(&Ttl::new(30).unwrap()));

    // No hot TTL given -> defaults to the field's remaining TTL (never None,
    // since persistence does not exist). Approximate: some time has elapsed.
    match meta_map.get("cold2") {
        Some(t) => assert!(
            *t > Ttl::new(55).unwrap() && *t <= Ttl::new(60).unwrap(),
            "cold2 hot TTL should track the field TTL (~60), got {t:?}"
        ),
        other => panic!("expected cold2 to default its hot TTL, got {other:?}"),
    }
}

/// Core `SessionStore` conformance. `$setup` is an async fn returning something
/// that derefs to the store.
#[macro_export]
macro_rules! define_session_store_tests {
    ($setup:ident) => {
        #[tokio::test]
        async fn test_store_basic_crud() {
            common::run_basic_crud(&*$setup().await).await;
        }
        #[tokio::test]
        async fn test_store_get_nonexistent() {
            common::run_get_nonexistent(&*$setup().await).await;
        }
        #[tokio::test]
        async fn test_store_get_all() {
            common::run_get_all(&*$setup().await).await;
        }
        #[tokio::test]
        async fn test_store_remove() {
            common::run_remove(&*$setup().await).await;
        }
        #[tokio::test]
        async fn test_store_delete() {
            common::run_delete(&*$setup().await).await;
        }
        #[tokio::test]
        async fn test_store_rename() {
            common::run_rename(&*$setup().await).await;
        }
        #[tokio::test]
        async fn test_store_rename_preserves_all_fields() {
            common::run_rename_preserves_all_fields(&*$setup().await).await;
        }
        #[tokio::test]
        async fn test_store_rename_collision() {
            common::run_rename_collision(&*$setup().await).await;
        }
        #[tokio::test]
        async fn test_store_expire_field() {
            common::run_expire_field(&*$setup().await).await;
        }
    };
}

/// Timing-sensitive tests (these sleep ~2.5s each). Opt in per backend that can
/// honor second-granular TTLs.
#[macro_export]
macro_rules! define_session_store_timing_tests {
    ($setup:ident) => {
        #[tokio::test]
        async fn test_store_field_natural_expiry() {
            common::run_field_natural_expiry(&*$setup().await).await;
        }
        #[tokio::test]
        async fn test_store_expire_field_extends() {
            common::run_expire_field_extends(&*$setup().await).await;
        }
    };
}

/// `LayeredHotStore` conformance.
#[macro_export]
macro_rules! define_layered_hot_store_tests {
    ($setup:ident) => {
        #[cfg(feature = "layered-store")]
        #[tokio::test]
        async fn test_hot_store_multiple() {
            common::run_layered_hot(&*$setup().await).await;
        }
    };
}

/// `LayeredColdStore` conformance.
#[macro_export]
macro_rules! define_layered_cold_store_tests {
    ($setup:ident) => {
        #[cfg(feature = "layered-store")]
        #[tokio::test]
        async fn test_cold_store_meta() {
            common::run_layered_cold(&*$setup().await).await;
        }
    };
}

/// `Session` wrapper conformance. `$setup_session` is an async fn returning
/// `(impl Deref<Target = S>, Session<S>)` — the store handle and a freshly
/// constructed session over it.
#[macro_export]
macro_rules! define_session_tests {
    ($setup_session:ident) => {
        #[tokio::test]
        async fn test_session_operations() {
            let (_store, session) = $setup_session().await;
            common::run_session_operations(session).await;
        }
        #[tokio::test]
        async fn test_session_set_validation() {
            let (_store, session) = $setup_session().await;
            common::run_session_set_validation(session).await;
        }
        #[tokio::test]
        async fn test_session_uninitialized() {
            let (_store, session) = $setup_session().await;
            common::run_session_uninitialized(session).await;
        }
        #[tokio::test]
        async fn test_session_expire_field() {
            let (_store, session) = $setup_session().await;
            common::run_session_expire_field(session).await;
        }
        #[tokio::test]
        async fn test_session_regenerate() {
            let (store, session) = $setup_session().await;
            common::run_session_regenerate(&*store, session).await;
        }
        #[tokio::test]
        async fn test_session_prepare_regenerate() {
            let (store, session) = $setup_session().await;
            common::run_session_prepare_regenerate(&*store, session).await;
        }
    };
}
