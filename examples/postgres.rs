//! This example requires the `postgres-store` feature.
//!
//! It demonstrates how to set up and use the `PostgresStore`.
//!
//! To run this example, you need a Postgres database running and a `DATABASE_URL`
//! environment variable set, for example:
//! `DATABASE_URL=postgres://user:password@localhost:5432/database`

use axum::routing::get;
use axum::{Json, Router};
use ruts::store::postgres::{PostgresStore, PostgresStoreBuilder};
use ruts::{CookieOptions, Session, SessionLayer};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use tower_cookies::CookieManagerLayer;

type PostgresSession = Session<PostgresStore>;

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
        .route(
            "/insert",
            get(|session: PostgresSession| async move {
                let app_session = AppSession {
                    user: Some(User {
                        id: 34895634,
                        name: "John Doe".to_string(),
                    }),
                    ip: Some(IpAddr::from(Ipv4Addr::new(192, 168, 0, 1))),
                    theme: Some(Theme::Dark),
                };

                session
                    .insert("app", &app_session, None)
                    .await
                    .expect("Failed to insert session data");
            }),
        )
        .route(
            "/get",
            get(|session: PostgresSession| async move {
                let app_session: Option<AppSession> = session
                    .get("app")
                    .await
                    .expect("Failed to get session data");
                Json(app_session.unwrap())
            }),
        )
        .route(
            "/delete",
            get(|session: PostgresSession| async move {
                session.delete().await.expect("Failed to delete session");
            }),
        )
}

#[tokio::main]
async fn main() {
    // 1. Set up your database connection pool.
    let database_url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this example");
    let pool = PgPool::connect(&database_url)
        .await
        .expect("Failed to connect to database");

    // 2. Create the session store using the builder.
    let store = PostgresStoreBuilder::new(pool)
        .schema_name("my_schema")
        .table_name("my_sessions")
        .cleanup_interval(tokio::time::Duration::from_secs(60))
        .build()
        .await
        .expect("Failed to build PostgresStore");

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

    // Set up router with session management
    let app = routes()
        .layer(session_layer)
        .layer(CookieManagerLayer::new());

    // Run the server
    let listener = tokio::net::TcpListener::bind("0.0.0.0:9000").await.unwrap();
    println!("Listening on http://0.0.0.0:9000");
    axum::serve(listener, app).await.unwrap();
}
