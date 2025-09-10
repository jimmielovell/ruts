//! This example requires the `layered-store`, `redis-store`, and `postgres-store` features.
//!
//! To run this example, you need Redis and Postgres running, and the following
//! environment variables set:
//! `DATABASE_URL=postgres://user:password@localhost:5432/database`
//! `REDIS_URL=redis://127.0.0.1/`

use axum::routing::get;
use axum::{Json, Router};
use fred::clients::Client;
use fred::interfaces::ClientLike;
use ruts::store::layered::{LayeredStore, LayeredWriteStrategy};
use ruts::store::postgres::{PostgresStore, PostgresStoreBuilder};
use ruts::store::redis::RedisStore;
use ruts::{CookieOptions, Session, SessionLayer};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use tower_cookies::CookieManagerLayer;

type LayeredSession = Session<LayeredStore<RedisStore<Client>, PostgresStore>>;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
struct User {
    id: i64,
    name: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
enum Theme {
    Light,
    #[default]
    Dark,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
struct AppSession {
    user: Option<User>,
    ip: Option<IpAddr>,
    theme: Option<Theme>,
}

fn routes() -> Router<()> {
    Router::new()
        // default write-through strategy.
        .route(
            "/insert_default",
            get(|session: LayeredSession| async move {
                let app_session = AppSession {
                    user: Some(User {
                        id: 1,
                        name: "Default User".into(),
                    }),
                    ..Default::default()
                };
                // This will be written to both Redis and Postgres.
                session.insert("app", &app_session, None).await.unwrap();
            }),
        )
        // cap the TTL on the hot cache.
        .route(
            "/insert_capped_ttl",
            get(|session: LayeredSession| async move {
                let user = User {
                    id: 2,
                    name: "Capped TTL User".into(),
                };
                let long_term_expiry = 60 * 60 * 24 * 30; // 1 month in Postgres
                let short_term_hot_cache_expiry = 60; // 1 minute in Redis

                let strategy =
                    LayeredWriteStrategy::WriteThrough(user, short_term_hot_cache_expiry);

                session
                    .update("user", &strategy, Some(long_term_expiry))
                    .await
                    .unwrap();
            }),
        )
        // write only to the cold store.
        .route(
            "/insert_cold_only",
            get(|session: LayeredSession| async move {
                let user = User {
                    id: 3,
                    name: "Cold Only User".into(),
                };
                let strategy = LayeredWriteStrategy::ColdCache(user);

                // This will be written to Postgres, but NOT to Redis.
                session.update("user", &strategy, None).await.unwrap();
            }),
        )
        // test cache-aside behavior.
        .route(
            "/get",
            get(|session: LayeredSession| async move {
                let app_session: Option<AppSession> = session
                    .get("app")
                    .await
                    .expect("Failed to get session data");
                let user_session: Option<User> =
                    session.get("user").await.expect("Failed to get user data");

                Json((app_session, user_session))
            }),
        )
}

#[tokio::main]
async fn main() {
    // 1. Set up Redis client (Hot Cache)
    let redis_url = std::env::var("REDIS_URL").expect("REDIS_URL must be set");
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
    let cold_store = PostgresStoreBuilder::new(pool)
        .build()
        .await
        .expect("Failed to build PostgresStore");

    // 3. Create the LayeredStore
    let store = LayeredStore::new(hot_store, cold_store);

    // Configure session options
    let cookie_options = CookieOptions::build()
        .name("session")
        .http_only(true)
        .same_site(ruts::cookie::SameSite::Lax)
        .secure(false) // Use `true` in production
        .max_age(60 * 60) // 1 hour
        .path("/");

    // Create session layer
    let session_layer = SessionLayer::new(Arc::new(store)).with_cookie_options(cookie_options);
    let app = routes()
        .layer(session_layer)
        .layer(CookieManagerLayer::new());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:9002").await.unwrap();
    println!("Listening on http://0.0.0.0:9002");
    axum::serve(listener, app).await.unwrap();
}
