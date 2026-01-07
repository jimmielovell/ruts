//! This example requires the `layered-store`, `redis-store`, and `postgres-store` features.
//!
//! To run this example, you need Redis and Postgres running, and the following
//! environment variables set:
//! `DATABASE_URL=postgres://user:password@localhost:5432/database`

use axum::routing::get;
use axum::{Json, Router};
use fred::clients::Client;
use fred::interfaces::ClientLike;
use ruts::store::layered::LayeredStore;
use ruts::store::postgres::{PostgresStore, PostgresStoreBuilder};
use ruts::store::redis::RedisStore;
use ruts::{CookieOptions, Session, SessionLayer};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::sync::Arc;
use tower_cookies::CookieManagerLayer;

type LayeredSession = Session<LayeredStore<RedisStore<Client>, PostgresStore>>;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
struct User {
    id: i64,
    name: String,
}

fn routes() -> Router<()> {
    Router::new()
        .route(
            "/set",
            get(|session: LayeredSession| async move {
                let user = User {
                    id: 1,
                    name: "Default User".into(),
                };
                // This will be written to both Redis and Postgres.
                session.set("user_1", &user, None, None).await.unwrap();
            }),
        )
        // cap the TTL on the hot cache.
        .route(
            "/set_hot_ttl",
            get(|session: LayeredSession| async move {
                let user = User {
                    id: 2,
                    name: "Capped Hot TTL User".into(),
                };
                let long_term_expiry = 60 * 10; // 10 minutes in Postgres
                let short_term_hot_cache_expiry = 60; // 1 minute in Redis

                session
                    .set(
                        "user_2",
                        &user,
                        Some(long_term_expiry),
                        Some(short_term_hot_cache_expiry),
                    )
                    .await
                    .unwrap();
            }),
        )
        // write only to the cold store.
        .route(
            "/set_cold_only",
            get(|session: LayeredSession| async move {
                let user = User {
                    id: 3,
                    name: "Cold Only User".into(),
                };
                // This will be written to Postgres, but NOT to Redis.
                session.set("user_3", &user, None, Some(0)).await.unwrap();
            }),
        )
        .route(
            "/remove_set",
            get(|session: LayeredSession| async move {
                session.remove("user_1").await.unwrap();
                let user = User {
                    id: 1,
                    name: "Removed User".into(),
                };
                session.prepare_regenerate();
                // This will be written to both Redis and Postgres.
                session.set("user_4", &user, None, None).await.unwrap();
            }),
        )
        .route(
            "/get",
            get(|session: LayeredSession| async move {
                let user_1: Option<User> = session.get("user_1").await.unwrap_or_default();
                let user_2: Option<User> = session.get("user_2").await.unwrap_or_default();
                let user_3: Option<User> = session.get("user_3").await.unwrap_or_default();

                Json((user_1, user_2, user_3))
            }),
        )
}

#[tokio::main]
async fn main() {
    // 1. Set up Redis client (Hot Cache)
    let redis_client = Client::default();
    redis_client
        .init()
        .await
        .expect("Failed to connect to Redis");
    let hot_store = RedisStore::new(Arc::new(redis_client));

    // 2. Set up Postgres pool (Cold Store)
    let database_url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this example");
    let pool = PgPool::connect(&database_url)
        .await
        .expect("Failed to connect to database");
    let cold_store = PostgresStoreBuilder::new(pool, true)
        .build()
        .await
        .expect("Failed to build PostgresStore");

    // 3. Create the LayeredStore
    let store = LayeredStore::new(hot_store, cold_store);

    // Configure session options
    let cookie_options = CookieOptions::build()
        .name("session")
        .http_only(true)
        .same_site(cookie::SameSite::Lax)
        .secure(false) // Use `true` in production
        .max_age(60 * 10) // 10 minutes
        .path("/");

    // Create session layer
    let session_layer = SessionLayer::new(Arc::new(store)).with_cookie_options(cookie_options);
    let app = routes()
        .layer(session_layer)
        .layer(CookieManagerLayer::new());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:9000").await.unwrap();
    println!("Listening on http://0.0.0.0:9000");
    axum::serve(listener, app).await.unwrap();
}
