mod common;

use common::TestData;
use fred::clients::Client;
use fred::interfaces::ClientLike;
use ruts::Id;
use ruts::store::layered::LayeredStore;
use ruts::store::postgres::{PostgresStore, PostgresStoreBuilder};
use ruts::store::redis::RedisStore;
use ruts::store::{SessionStore, Ttl};
use sqlx::PgPool;
use std::sync::Arc;

type Hot = RedisStore<Client>;
type Store = LayeredStore<Hot, PostgresStore>;

async fn setup_parts() -> (Hot, Arc<Store>) {
    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = PgPool::connect(&database_url).await.unwrap();

    sqlx::query("drop table if exists t_layered cascade")
        .execute(&pool)
        .await
        .unwrap();

    let cold = PostgresStoreBuilder::new(pool)
        .table_name("t_layered")
        .unwrap()
        .create_table(true)
        .build()
        .await
        .unwrap();

    // Configurable because this suite calls FLUSHALL: pointing it at the wrong
    // instance wipes that instance.
    let client = match std::env::var("REDIS_URL") {
        Ok(url) => {
            let config = fred::types::config::Config::from_url(&url).unwrap();
            Client::new(config, None, None, None)
        }
        Err(_) => Client::default(),
    };
    let _ = client.connect();
    client.wait_for_connect().await.unwrap();
    let _: Result<(), fred::error::Error> = client.flushall(false).await;

    let hot = RedisStore::new(Arc::new(client)).await.unwrap();

    (hot.clone(), Arc::new(LayeredStore::new(hot, cold)))
}

async fn setup_store() -> Arc<Store> {
    setup_parts().await.1
}

define_session_store_tests!(setup_store);

/// A zero hot-cache TTL means "keep this out of the hot store" — the field is
/// still persisted, still readable, but never cached. The write must not put it
/// there, and neither may the cache warming that follows a later read.
///
/// The warming path is the sharp edge: it hands the hot store every field of
/// the session at once, so one field marked "never cache" used to either be
/// cached anyway or fail the whole read.
#[tokio::test]
async fn test_layered_zero_hot_ttl_is_never_cached() {
    let (hot, store) = setup_parts().await;
    let id = Id::default();
    let field_ttl = Ttl::new(60).unwrap();
    let cached = TestData {
        f1: 1,
        f2: "cached".into(),
    };
    let never = TestData {
        f1: 2,
        f2: "never".into(),
    };

    store
        .set(
            &id,
            "cached",
            &cached,
            field_ttl,
            Some(Ttl::new(30).unwrap()),
        )
        .await
        .unwrap();
    store
        .set(&id, "never", &never, field_ttl, Some(Ttl::ZERO))
        .await
        .unwrap();

    assert!(
        hot.get::<TestData>(&id, "never").await.unwrap().is_none(),
        "a zero hot TTL must not write the field to the hot store"
    );

    // It is persisted all the same, and reads find it in the cold store.
    assert_eq!(
        store.get::<TestData>(&id, "never").await.unwrap(),
        Some(never),
        "a zero hot TTL must not stop the field being stored"
    );

    // That read warmed the cache from cold. The cacheable field landed there;
    // the one marked "never cache" did not.
    assert!(
        hot.get::<TestData>(&id, "cached").await.unwrap().is_some(),
        "warming must still cache the fields that allow it"
    );
    assert!(
        hot.get::<TestData>(&id, "never").await.unwrap().is_none(),
        "warming must not cache a field the cold store marked \"never cache\""
    );
}

/// A field inside its last second is reported by the cold store with a zero hot
/// TTL, for the same reason: there is nothing worth caching. The read that
/// finds it must still succeed.
#[tokio::test]
async fn test_layered_read_survives_a_nearly_expired_field() {
    let (hot, store) = setup_parts().await;
    let id = Id::default();
    let data = common::create_test_data();

    store
        .set(&id, "keep", &data, Ttl::new(60).unwrap(), None)
        .await
        .unwrap();
    store
        .set(&id, "doomed", &data, Ttl::new(1).unwrap(), None)
        .await
        .unwrap();

    // Drop the hot copies so the next read has to go to cold and warm from it.
    hot.delete(&id).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;

    assert_eq!(
        store.get::<TestData>(&id, "keep").await.unwrap(),
        Some(data),
        "a sibling field in its last second must not fail the read"
    );
}

/// Zero on the field TTL is the other axis: nothing is stored in either tier.
#[tokio::test]
async fn test_layered_zero_field_ttl_clears_both_tiers() {
    let (hot, store) = setup_parts().await;
    let id = Id::default();
    let ttl = Ttl::new(60).unwrap();
    let data = common::create_test_data();

    store
        .set(&id, "doomed", &data, ttl, Some(ttl))
        .await
        .unwrap();
    store.set(&id, "keep", &data, ttl, Some(ttl)).await.unwrap();
    assert!(hot.get::<TestData>(&id, "doomed").await.unwrap().is_some());

    store
        .set(&id, "doomed", &data, Ttl::ZERO, None)
        .await
        .unwrap();

    assert!(
        hot.get::<TestData>(&id, "doomed").await.unwrap().is_none(),
        "a zero field TTL must clear the hot copy"
    );
    assert!(
        store
            .get::<TestData>(&id, "doomed")
            .await
            .unwrap()
            .is_none(),
        "a zero field TTL must clear the cold copy too"
    );
    assert!(store.get::<TestData>(&id, "keep").await.unwrap().is_some());
}
