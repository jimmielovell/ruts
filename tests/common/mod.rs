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

    // Removing a missing field (and a missing session) is a no-op, not an
    // error, and reports that there was nothing to remove.
    assert!(
        !store.remove(&id, "nope").await.unwrap(),
        "removing from a session that does not exist must report false"
    );

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

    assert!(
        !store.remove(&id, "missing").await.unwrap(),
        "removing a field the session does not have must report false"
    );

    assert!(
        store.remove(&id, "a").await.unwrap(),
        "removing a live field must report that it was there"
    );
    assert!(store.get::<TestData>(&id, "a").await.unwrap().is_none());
    assert!(
        store.get::<TestData>(&id, "b").await.unwrap().is_some(),
        "removing one field must not affect the others"
    );
    assert!(
        !store.remove(&id, "a").await.unwrap(),
        "removing the same field twice must report false the second time"
    );

    assert!(store.remove(&id, "b").await.unwrap());
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

/// After a rotation the previous id must be inert — for *every* operation, not
/// just reads. A superseded id still carries whatever the client had, so each
/// destructive path has to be checked separately: it was `delete` that turned
/// out to be reachable last time, while reads were fine.
pub async fn run_stale_id_is_inert<S: SessionStore>(store: &S) {
    let old = Id::default();
    let new = Id::default();
    let original = create_test_data();
    let ttl = Ttl::new(60).unwrap();

    helper_set(store, &old, "auth", &original, ttl, Some(ttl))
        .await
        .unwrap();
    assert!(store.rename_session_id(&old, &new).await.unwrap());

    // The rotated id still reaches the session.
    assert_eq!(
        store.get::<TestData>(&new, "auth").await.unwrap(),
        Some(original.clone())
    );

    // Reads through the stale id find nothing.
    assert!(
        store.get::<TestData>(&old, "auth").await.unwrap().is_none(),
        "a stale id must not read the session"
    );
    assert!(store.get_all(&old).await.unwrap().is_none());

    // It cannot extend a field's life either.
    assert!(
        !store.expire_field(&old, "auth", ttl).await.unwrap(),
        "a stale id must not be able to refresh a live field"
    );

    // Writing through it must not reach the live session. On a store that
    // indirects this establishes a *new*, empty session under the stale id —
    // the same thing any unauthenticated visitor gets — rather than touching
    // the one that was rotated away.
    let poison = TestData {
        f1: 666,
        f2: "poison".into(),
    };
    helper_set(store, &old, "auth", &poison, ttl, Some(ttl))
        .await
        .unwrap();
    assert_eq!(
        store.get::<TestData>(&new, "auth").await.unwrap(),
        Some(original.clone()),
        "a write through a stale id must not reach the live session"
    );

    // Nor may it remove from the live session.
    store.remove(&old, "auth").await.unwrap();
    assert_eq!(
        store.get::<TestData>(&new, "auth").await.unwrap(),
        Some(original.clone()),
        "remove through a stale id must not delete from the live session"
    );

    // Nor destroy it. This is the one that regressed before.
    store.delete(&old).await.unwrap();
    assert_eq!(
        store.get::<TestData>(&new, "auth").await.unwrap(),
        Some(original),
        "delete through a stale id must not destroy the live session"
    );
}

/// Rotating repeatedly must kill *every* prior id, not merely the one it just
/// replaced. A store that only unhooks the immediately-previous id would leave
/// a trail of working cookies behind it.
pub async fn run_rotation_chain_kills_every_prior_id<S: SessionStore>(store: &S) {
    let data = create_test_data();
    let ttl = Ttl::new(60).unwrap();

    let mut ids = vec![Id::default()];
    helper_set(store, &ids[0], "auth", &data, ttl, Some(ttl))
        .await
        .unwrap();

    for _ in 0..5 {
        let next = Id::default();
        assert!(
            store
                .rename_session_id(ids.last().unwrap(), &next)
                .await
                .unwrap()
        );
        ids.push(next);
    }

    let live = ids.last().unwrap();
    assert_eq!(
        store.get::<TestData>(live, "auth").await.unwrap(),
        Some(data),
        "the session must survive the whole chain"
    );

    for (generation, stale) in ids[..ids.len() - 1].iter().enumerate() {
        assert!(
            store
                .get::<TestData>(stale, "auth")
                .await
                .unwrap()
                .is_none(),
            "generation {generation} still reads the session after {} rotations",
            ids.len() - 1
        );
        assert!(
            store.get_all(stale).await.unwrap().is_none(),
            "generation {generation} still enumerates the session"
        );
    }
}

/// Two sessions must never reach each other's data, through any operation.
pub async fn run_sessions_are_isolated<S: SessionStore>(store: &S) {
    let a = Id::default();
    let b = Id::default();
    let ttl = Ttl::new(60).unwrap();

    let secret_a = TestData {
        f1: 1,
        f2: "a".into(),
    };
    let secret_b = TestData {
        f1: 2,
        f2: "b".into(),
    };

    helper_set(store, &a, "secret", &secret_a, ttl, Some(ttl))
        .await
        .unwrap();
    helper_set(store, &b, "secret", &secret_b, ttl, Some(ttl))
        .await
        .unwrap();

    assert_eq!(
        store.get::<TestData>(&a, "secret").await.unwrap(),
        Some(secret_a.clone())
    );
    assert_eq!(
        store.get::<TestData>(&b, "secret").await.unwrap(),
        Some(secret_b.clone())
    );

    // Rotating one must leave the other alone.
    let a2 = Id::default();
    assert!(store.rename_session_id(&a, &a2).await.unwrap());
    assert_eq!(
        store.get::<TestData>(&b, "secret").await.unwrap(),
        Some(secret_b.clone()),
        "rotating one session must not disturb another"
    );

    // Removing a field from one must leave the other alone.
    store.remove(&a2, "secret").await.unwrap();
    assert_eq!(
        store.get::<TestData>(&b, "secret").await.unwrap(),
        Some(secret_b.clone()),
        "removing a field from one session must not touch another"
    );

    // Deleting one must leave the other alone.
    store.delete(&a2).await.unwrap();
    assert_eq!(
        store.get::<TestData>(&b, "secret").await.unwrap(),
        Some(secret_b),
        "deleting one session must not destroy another"
    );
}

/// Concurrent first-writes through a single presented id.
///
/// This is the case we deliberately left unguarded: the store establishes a
/// session without a conditional insert, so racing creations are possible. The
/// assertion records what each backend actually does rather than presuming, and
/// would flip from documenting divergence to asserting convergence if a
/// conditional create were ever added.
pub async fn run_concurrent_creation<S: SessionStore>(store: &S) {
    const WRITERS: usize = 8;
    let ttl = Ttl::new(60).unwrap();

    for round in 0..8 {
        // Never yet established, which is exactly the state a lapsed cookie
        // presents: something that parses but resolves to nothing.
        let id = Id::default();

        let mut handles = Vec::with_capacity(WRITERS);
        for writer in 0..WRITERS {
            let store = store.clone();
            let id = id.clone();
            handles.push(tokio::spawn(async move {
                helper_set(
                    &store,
                    &id,
                    &format!("f{writer}"),
                    &(writer as i64),
                    ttl,
                    Some(ttl),
                )
                .await
            }));
        }
        for handle in handles {
            handle.await.unwrap().unwrap();
        }

        // Read back through a *fresh* id, so the answer comes from the store
        // rather than from anything cached on the id the writers shared.
        let fresh: Id = id.to_string().parse().unwrap();
        let survived = store
            .get_all(&fresh)
            .await
            .unwrap()
            .map(|map| map.len())
            .unwrap_or(0);

        assert_eq!(
            survived, WRITERS,
            "round {round}: {survived} of {WRITERS} concurrent first-writes \
             survived; racing creations orphaned the rest"
        );
    }
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

/// A session must stay reachable for as long as it has a live field, through an
/// id presented fresh on every request rather than the one the writing code
/// happened to be holding.
///
/// The distinction matters for a store that records *where* a session lives: the
/// id that did the writing remembers the answer, so it keeps working even after
/// the record is gone. Only an id parsed from the cookie again — which is what
/// the next request hands the store — has to go and look. A store that lets that
/// record lapse on the horizon of the write that established the session logs an
/// active user out while their data sits there, still live.
pub async fn run_session_outlives_its_first_write<S: SessionStore>(store: &S) {
    let id = Id::default();
    let cookie = id.to_string();
    let short = Ttl::new(2).unwrap();
    let long = Ttl::new(60).unwrap();

    // The write that establishes the session has the shorter horizon of the two.
    helper_set(store, &id, "short", &1i64, short, Some(short))
        .await
        .unwrap();
    helper_set(store, &id, "keep", &42i64, long, Some(long))
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(3000)).await;

    let next_request: Id = cookie.parse().unwrap();

    assert_eq!(
        store.get::<i64>(&next_request, "keep").await.unwrap(),
        Some(42),
        "the session must outlive the horizon of the write that established it"
    );
    assert!(
        store.get_all(&next_request).await.unwrap().is_some(),
        "the session must still enumerate through a freshly presented id"
    );
    assert!(
        store
            .get::<i64>(&next_request, "short")
            .await
            .unwrap()
            .is_none(),
        "the short-lived field itself must still lapse on time"
    );
}

/// A field that has lapsed is gone as far as every read is concerned, so
/// removing it must report that there was nothing to remove — even on a backend
/// whose row is still physically present because nothing has swept it yet.
pub async fn run_remove_reports_false_for_a_lapsed_field<S: SessionStore>(store: &S) {
    let id = Id::default();

    helper_set(
        store,
        &id,
        "brief",
        &create_test_data(),
        Ttl::new(1).unwrap(),
        None,
    )
    .await
    .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(2500)).await;

    assert!(
        !store.remove(&id, "brief").await.unwrap(),
        "removing a field that already lapsed must report false"
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

    // Zero says "never cache this field"; it must survive the round trip as
    // zero rather than being clamped up into a brief cache entry.
    store
        .set_with_meta(
            &session_id,
            "never_cache",
            &TestData {
                f1: 14,
                f2: "3".into(),
            },
            Ttl::new(60).unwrap(),
            Some(Ttl::ZERO),
        )
        .await
        .unwrap();

    let (session_map, meta_map) = store.get_all_with_meta(&session_id).await.unwrap().unwrap();

    assert_eq!(session_map.len(), 3);
    assert_eq!(meta_map.get("never_cache"), Some(&Ttl::ZERO));

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

/// `Ttl::ZERO` means "do not store this value". Every backend has to agree on
/// it: the field must not be readable afterwards, an existing one must be
/// cleared rather than left behind, and nothing may be established to hold a
/// value that is being discarded.
pub async fn run_zero_ttl_does_not_store<S: SessionStore>(store: &S) {
    let live = Ttl::new(60).unwrap();

    // A fresh field: nothing is stored, and no session is established for it.
    let id = Id::default();
    helper_set(store, &id, "f", &create_test_data(), Ttl::ZERO, None)
        .await
        .unwrap();
    assert!(
        store.get::<TestData>(&id, "f").await.unwrap().is_none(),
        "a zero TTL must not store the value"
    );
    assert!(
        store.get_all(&id).await.unwrap().is_none(),
        "a zero TTL must not establish a session"
    );

    // An existing field: the write clears it instead of leaving the old value
    // readable under its own, longer horizon.
    let id = Id::default();
    helper_set(store, &id, "doomed", &create_test_data(), live, None)
        .await
        .unwrap();
    helper_set(store, &id, "keep", &create_test_data(), live, None)
        .await
        .unwrap();

    helper_set(
        store,
        &id,
        "doomed",
        &TestData {
            f1: 99,
            f2: "replacement".into(),
        },
        Ttl::ZERO,
        None,
    )
    .await
    .unwrap();

    assert!(
        store
            .get::<TestData>(&id, "doomed")
            .await
            .unwrap()
            .is_none(),
        "a zero TTL must clear a field that was already live"
    );
    assert!(
        store.get::<TestData>(&id, "keep").await.unwrap().is_some(),
        "a zero TTL must not touch the session's other fields"
    );
}

/// The rotation in `set_and_rename` stands on its own: a zero TTL discards the
/// value being written without cancelling the rename.
pub async fn run_zero_ttl_still_renames<S: SessionStore>(store: &S) {
    let live = Ttl::new(60).unwrap();
    let old = Id::default();
    let new = Id::default();
    let carried = TestData {
        f1: 4,
        f2: "carried".into(),
    };

    helper_set(store, &old, "keep", &carried, live, None)
        .await
        .unwrap();
    helper_set(store, &old, "doomed", &create_test_data(), live, None)
        .await
        .unwrap();

    helper_set_and_rename(
        store,
        &old,
        &new,
        "doomed",
        &create_test_data(),
        Ttl::ZERO,
        None,
    )
    .await
    .unwrap();

    assert_eq!(
        store.get::<TestData>(&new, "keep").await.unwrap(),
        Some(carried),
        "the rename must still happen when the field is discarded"
    );
    assert!(
        store
            .get::<TestData>(&new, "doomed")
            .await
            .unwrap()
            .is_none(),
        "the discarded field must not survive the rename"
    );
    assert!(
        store.get_all(&old).await.unwrap().is_none(),
        "the old id must be inert after the rename"
    );

    // With no session to rotate, nothing is created for the discarded value.
    let ghost = Id::default();
    let target = Id::default();
    helper_set_and_rename(
        store,
        &ghost,
        &target,
        "f",
        &create_test_data(),
        Ttl::ZERO,
        None,
    )
    .await
    .unwrap();
    assert!(store.get_all(&target).await.unwrap().is_none());
    assert!(store.get_all(&ghost).await.unwrap().is_none());
}

/// `expire_field` with a zero TTL removes the field rather than giving it a new
/// horizon, and still reports whether there was anything there to remove.
pub async fn run_zero_ttl_expire_field_removes<S: SessionStore>(store: &S) {
    let live = Ttl::new(60).unwrap();
    let id = Id::default();

    helper_set(store, &id, "doomed", &create_test_data(), live, None)
        .await
        .unwrap();
    helper_set(store, &id, "keep", &create_test_data(), live, None)
        .await
        .unwrap();

    assert!(
        store.expire_field(&id, "doomed", Ttl::ZERO).await.unwrap(),
        "expiring a live field with a zero TTL must report that it was there"
    );
    assert!(
        store
            .get::<TestData>(&id, "doomed")
            .await
            .unwrap()
            .is_none(),
        "a zero TTL must remove the field"
    );
    assert!(
        store.get::<TestData>(&id, "keep").await.unwrap().is_some(),
        "it must not touch the session's other fields"
    );

    // Nothing there to remove -> false, and nothing is created.
    assert!(!store.expire_field(&id, "doomed", Ttl::ZERO).await.unwrap());
    assert!(!store.expire_field(&id, "ghost", Ttl::ZERO).await.unwrap());
    assert!(store.get::<TestData>(&id, "ghost").await.unwrap().is_none());

    let none_id = Id::default();
    assert!(!store.expire_field(&none_id, "f", Ttl::ZERO).await.unwrap());
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
        async fn test_store_stale_id_is_inert() {
            common::run_stale_id_is_inert(&*$setup().await).await;
        }
        #[tokio::test]
        async fn test_store_rotation_chain_kills_every_prior_id() {
            common::run_rotation_chain_kills_every_prior_id(&*$setup().await).await;
        }
        #[tokio::test]
        async fn test_store_sessions_are_isolated() {
            common::run_sessions_are_isolated(&*$setup().await).await;
        }
        #[tokio::test]
        async fn test_store_concurrent_creation() {
            common::run_concurrent_creation(&*$setup().await).await;
        }
        #[tokio::test]
        async fn test_store_expire_field() {
            common::run_expire_field(&*$setup().await).await;
        }
        #[tokio::test]
        async fn test_store_zero_ttl_does_not_store() {
            common::run_zero_ttl_does_not_store(&*$setup().await).await;
        }
        #[tokio::test]
        async fn test_store_zero_ttl_still_renames() {
            common::run_zero_ttl_still_renames(&*$setup().await).await;
        }
        #[tokio::test]
        async fn test_store_zero_ttl_expire_field_removes() {
            common::run_zero_ttl_expire_field_removes(&*$setup().await).await;
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
        async fn test_store_remove_reports_false_for_a_lapsed_field() {
            common::run_remove_reports_false_for_a_lapsed_field(&*$setup().await).await;
        }
        #[tokio::test]
        async fn test_store_expire_field_extends() {
            common::run_expire_field_extends(&*$setup().await).await;
        }
        #[tokio::test]
        async fn test_store_session_outlives_its_first_write() {
            common::run_session_outlives_its_first_write(&*$setup().await).await;
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
