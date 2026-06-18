mod common;

use ruts::store::postgres::PostgresStoreBuilder;
use sqlx::PgPool;
use std::sync::Arc;

async fn setup_store() -> Arc<ruts::store::postgres::PostgresStore> {
    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
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
