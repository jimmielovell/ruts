#[cfg(test)]
mod tests {
    use fred::clients::Client;
    use fred::prelude::ClientLike;
    use ruts::Id;
    use ruts::store::SessionStore;
    use ruts::store::redis::RedisStore;
    use std::sync::Arc;
    use tokio::time::{Duration, sleep};

    async fn setup_store() -> RedisStore<Client> {
        let client = Client::default();
        client.connect();
        client.wait_for_connect().await.unwrap();
        RedisStore::new(Arc::new(client))
    }

    #[tokio::test]
    async fn test_insert_and_get() {
        let store = setup_store().await;
        let sid = Id::default();

        let ttl = store
            .insert(&sid, "field1", &"value1", Some(10), None)
            .await
            .unwrap();
        assert_eq!(ttl, 10);

        let v: Option<String> = store.get(&sid, "field1").await.unwrap();
        assert_eq!(v.unwrap(), "value1");
    }

    #[tokio::test]
    async fn test_insert_with_field_ttl() {
        let store = setup_store().await;
        let sid = Id::default();

        store
            .insert(&sid, "f", &"temp", None, Some(1))
            .await
            .unwrap();

        // Initially exists
        let v: Option<String> = store.get(&sid, "f").await.unwrap();
        assert_eq!(v.unwrap(), "temp");

        // Should expire after 1s
        sleep(Duration::from_secs(2)).await;
        let v: Option<String> = store.get(&sid, "f").await.unwrap();
        assert!(v.is_none());
    }

    #[tokio::test]
    async fn test_update_with_persistent_field() {
        let store = setup_store().await;
        let sid = Id::default();

        let ttl = store
            .insert(&sid, "f", &"x", Some(5), Some(5))
            .await
            .unwrap();
        assert_eq!(ttl, 5);

        let ttl = store.update(&sid, "f", &"y", None, Some(-1)).await.unwrap();
        assert_eq!(ttl, -1);
    }

    #[tokio::test]
    async fn test_insert_with_rename_success() {
        let store = setup_store().await;
        let old_sid = Id::default();
        let new_sid = Id::default();

        store
            .insert(&old_sid, "f", &"foo", None, None)
            .await
            .unwrap();
        store
            .insert_with_rename(&old_sid, &new_sid, "g", &"bar", None, None)
            .await
            .unwrap();

        let v: Option<String> = store.get(&new_sid, "f").await.unwrap();
        assert_eq!(v.unwrap(), "foo");

        let v: Option<String> = store.get(&new_sid, "g").await.unwrap();
        assert_eq!(v.unwrap(), "bar");
    }

    #[tokio::test]
    async fn test_insert_with_rename_conflict() {
        let store = setup_store().await;
        let old_sid = Id::default();
        let new_sid = Id::default();

        store
            .insert(&old_sid, "f", &"foo", None, None)
            .await
            .unwrap();
        store
            .insert(&new_sid, "existing", &"bar", None, None)
            .await
            .unwrap();

        // RENAMENX should fail, but we still insert into new_sid
        store
            .insert_with_rename(&old_sid, &new_sid, "g", &"baz", None, None)
            .await
            .unwrap();

        let v: Option<String> = store.get(&new_sid, "g").await.unwrap();
        assert_eq!(v.unwrap(), "baz");
    }

    #[tokio::test]
    async fn test_remove_and_delete() {
        let store = setup_store().await;
        let sid = Id::default();

        store
            .insert(&sid, "f1", &"v1", Some(10), None)
            .await
            .unwrap();
        store
            .insert(&sid, "f2", &"v2", Some(10), None)
            .await
            .unwrap();

        let ttl = store.remove(&sid, "f1").await.unwrap();
        assert_eq!(ttl, 10); // still has key

        let ttl = store.remove(&sid, "f2").await.unwrap();
        assert_eq!(ttl, -2);
    }

    #[cfg(feature = "layered-store")]
    #[tokio::test]
    async fn test_update_many() {
        use ruts::store::LayeredHotStore;

        let store = setup_store().await;
        let sid = Id::default();

        let pairs = vec![
            ("a".to_string(), b"1".to_vec(), Some(10)),
            ("b".to_string(), b"2".to_vec(), Some(-1)), // persistent
        ];

        let ttl = store.update_many(&sid, &pairs).await.unwrap();
        assert_eq!(ttl, -1); // persistent because of "b"

        let v: Option<String> = store.get(&sid, "a").await.unwrap();
        assert_eq!(v.unwrap(), "1");

        let v: Option<String> = store.get(&sid, "b").await.unwrap();
        assert_eq!(v.unwrap(), "2");
    }
}
