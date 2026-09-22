mod common;

use common::TestData;
use ruts::Id;
use ruts::store::postgres::PostgresStoreBuilder;
use ruts::store::{SessionStore, Ttl};
use sqlx::PgPool;
use std::sync::Arc;

fn database_url() -> String {
    std::env::var("DATABASE_URL").expect("DATABASE_URL must be set")
}

async fn setup_store() -> Arc<ruts::store::postgres::PostgresStore> {
    let database_url = database_url();
    let pool = PgPool::connect(&database_url).await.unwrap();

    sqlx::query("drop table if exists t_sessions cascade")
        .execute(&pool)
        .await
        .unwrap();

    let store = PostgresStoreBuilder::new(pool)
        .create_table(true)
        .build()
        .await
        .unwrap();

    Arc::new(store)
}

define_session_store_tests!(setup_store);
define_layered_cold_store_tests!(setup_store);

/// Unlike the other backends, a lapsed field leaves a row behind here until the
/// cleanup task sweeps it — so `remove` can still see something the rest of the
/// store cannot. It has to answer about what was live, not about what is on
/// disk. Backdating the row keeps this exact rather than timing-dependent.
#[tokio::test]
async fn test_remove_reports_false_for_an_unswept_lapsed_row() {
    let store = setup_store().await;
    let pool = PgPool::connect(&database_url()).await.unwrap();
    let id = Id::default();

    store
        .set(
            &id,
            "brief",
            &common::create_test_data(),
            Ttl::new(60).unwrap(),
            None,
        )
        .await
        .unwrap();

    sqlx::query(
        "update t_sessions set expires_at = now() - interval '1 hour' where session_id = $1",
    )
    .bind(id.as_str())
    .execute(&pool)
    .await
    .unwrap();

    // Nothing can read it any more...
    assert!(store.get::<TestData>(&id, "brief").await.unwrap().is_none());

    // ...so there is nothing to remove, even though the row is still there.
    assert!(
        !store.remove(&id, "brief").await.unwrap(),
        "a lapsed row must not be reported as a removed field"
    );

    // And the row is reclaimed rather than left for the sweeper.
    let remaining: i64 =
        sqlx::query_scalar("select count(*) from t_sessions where session_id = $1 and field = $2")
            .bind(id.as_str())
            .bind("brief")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(remaining, 0, "the lapsed row should still be deleted");

    // `expire_field` with a zero TTL answers the same question the same way.
    assert!(!store.expire_field(&id, "brief", Ttl::ZERO).await.unwrap());
}
