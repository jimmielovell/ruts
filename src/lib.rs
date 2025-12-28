//! # Ruts: Rust Tower Session for HTTP Applications
//!
//! `ruts` is a powerful and flexible session management middleware for Rust's Tower web
//! framework, with a focus on performance, durability, and ergonomic design.
//!
//! # Quick Start
//!
//! Here's a basic example with [Axum](https://docs.rs/axum/latest/axum/) and the `RedisStore`.
//! This requires the `axum` (enabled by default) and `redis-store` features.
//!
//! ```rust,no_run
//! use axum::{Router, routing::get};
//! use ruts::{Session, SessionLayer, CookieOptions};
//! use ruts::store::redis::RedisStore;
//! use fred::clients::Client;
//! use std::sync::Arc;
//! use fred::interfaces::ClientLike;
//! use tower_cookies::CookieManagerLayer;
//!
//! #[tokio::main]
//! async fn main() {
//!     // Set up Redis client
//!     let client = Client::default();
//!     client.connect();
//!     client.wait_for_connect().await.unwrap();
//!
//!     // Create session store
//!     let store = RedisStore::new(Arc::new(client));
//!
//!     // Configure session-cookie options
//!     let cookie_options = CookieOptions::build()
//!         .name("session")
//!         .http_only(true)
//!         .same_site(cookie::SameSite::Lax)
//!         .secure(true)
//!         .max_age(3600) // 1 hour
//!         .path("/");
//!
//!     // Create session layer
//!     let session_layer = SessionLayer::new(Arc::new(store))
//!         .with_cookie_options(cookie_options);
//!
//!     // Set up router with session management
//!     let app = Router::new()
//!         .route("/", get(handler))
//!         .layer(session_layer)
//!         .layer(CookieManagerLayer::new()); // CookieManagerLayer must be after
//!
//!     // Run the server
//!     let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
//!     axum::serve(listener, app).await.unwrap();
//! }
//!
//! async fn handler(session: Session<RedisStore<Client>>) -> String {
//!     let count: Option<i32> = session.get("count").await.unwrap();
//!     let new_count = count.unwrap_or(0) + 1;
//!     session.set("count", &new_count, None, None).await.unwrap();
//!     format!("You've visited this page {} times", new_count)
//! }
//! ```
//!
//! # Session Management
//!
//! ## Basic Operations
//!
//! ```rust,no_run
//! use ruts::Session;
//! use ruts::store::SessionMap;
//! use ruts::store::memory::MemoryStore;
//!
//! #[derive(serde::Deserialize)]
//! struct User;
//!
//! async fn handler(session: Session<MemoryStore>) {
//! // Get a single field's data
//! let value: Option<User> = session.get("key").await.unwrap();
//!
//! // Get all session data as a map for lazy deserialization
//! let session_map: Option<SessionMap> = session.get_all().await.unwrap();
//! if let Some(map) = session_map {
//!     let user: Option<User> = map.get("user").unwrap();
//! }
//!
//! // Update existing data
//! session.set("key", &"new_value", None, None).await.unwrap();
//!
//! // Prepare a new session ID before an insert/update to prevent session fixation
//! let new_id = session.prepare_regenerate();
//! session.set("key", &"value_with_new_id", None, None).await.unwrap();
//!
//! // Remove a single field
//! session.remove("key").await.unwrap();
//!
//! // Delete the entire session
//! session.delete().await.unwrap();
//!
//! // Regenerate session ID for security
//! session.regenerate().await.unwrap();
//!
//! // Update the session's overall expiry time
//! session.expire(7200).await.unwrap();
//!
//! // Get the current session ID
//! let id = session.id();
//! # }
//! ```
//!
//! # Stores
//!
//! `ruts` offers several backend stores for session data, each enabled by a feature flag.
//!
//! ## Redis
//! A high-performance Redis-backed session store. Ideal for production use as a primary or caching layer.
//!
//! ### Requirements
//!
//! - The `redis-store` feature.
//! - Redis 7.4 or later (required for field-level expiration using `HEXPIRE`).
//!
//! ```rust,no_run
//! use std::sync::Arc;
//! use fred::clients::Client;
//! use ruts::store::redis::RedisStore;
//!
//! let fred_client_or_pool = Client::default();
//! let store = RedisStore::new(Arc::new(fred_client_or_pool));
//! ```
//!
//! ## Postgres
//! A durable, persistent session store backed by a Postgres database.
//!
//! ### Requirements
//!
//! - The `postgres-store` feature.
//!
//! ```rust,no_run
//! use std::sync::Arc;
//! use sqlx::PgPool;
//! use ruts::store::postgres::PostgresStoreBuilder;
//!
//! #[tokio::main]
//! async fn main() {
//!      // 1. Set up your database connection pool.
//!      let database_url = std::env::var("DATABASE_URL")
//!          .expect("DATABASE_URL must be set");
//!      let pool = PgPool::connect(&database_url).await.unwrap();
//!
//!      // 2. Create the session store using the builder.
//!      // This will also run a migration to create the `sessions` table.
//!      let store = PostgresStoreBuilder::new(pool, true)
//!          // Optionally, you can customize the schema and table name
//!          // .schema_name("my_app")
//!          // .table_name("user_sessions")
//!          .build()
//!          .await
//!          .unwrap();
//! # }
//! ```
//!
//! ## LayeredStore
//!
//! **Note**: Requires the `layered-store`, `redis-store`, and `postgres-store` features
//!
//! A composite store that layers a fast, ephemeral "hot" cache (like Redis) on top of a
//! slower, persistent "cold" store (like Postgres). It is designed for scenarios where
//! sessions can have long lifespans but should only occupy expensive cache memory when
//! actively being used, thus balancing performance and durability.
//!
//! ```rust,no_run
//! use ruts::store::redis::RedisStore;
//! use ruts::store::postgres::PostgresStore;
//! use fred::clients::Client;
//! use sqlx::PgPool;
//! use ruts::store::layered::LayeredStore;
//! use ruts::Session;
//!
//! // Define a type alias for your specific layered store setup
//! type MyLayeredStore = LayeredStore<RedisStore<Client>, PostgresStore>;
//! type MySession = Session<MyLayeredStore>;
//!
//! #[derive(serde::Serialize)]
//! struct User { id: i32 }
//!
//! async fn handler(session: MySession) {
//!     let user = User { id: 1 };
//!
//!     // This session field is valid for 1 month in the persistent store.
//!     let long_term_expiry = 60 * 60 * 24 * 30;
//!
//!     // However, we only want it to live in the hot cache (Redis) for 1 hour.
//!     let short_term_hot_cache_expiry = 60 * 60;
//!
//!     // The cold store (Postgres) will get the long-term expiry,
//!     // but the hot store (Redis) will be capped at the shorter TTL.
//!     session.set("user", &user, None, Some(short_term_hot_cache_expiry)).await.unwrap();
//! }
//! ```
//!
//! ## Serialization
//! Ruts supports two serialization backends for session data storage:
//!
//! - [`bincode`](https://crates.io/crates/bincode) (default) - Fast, compact binary serialization.
//! - [`rmp-serde`](https://crates.io/crates/rmp-serde) (MessagePack) - Cross-language compatible serialization.
//!
//! To use `MessagePack` instead of the default `bincode`, add this to your `Cargo.toml`:
//!
//! ```toml
//! [dependencies]
//! ruts = { version = "0.7.1", default-features = false, features = ["axum", "messagepack"] }
//! ```
//!
//! ## Cookie Configuration
//!
//! ```rust
//! use ruts::CookieOptions;
//! use ruts::cookie::SameSite;
//! let cookie_options = CookieOptions::build()
//!     .name("my_session_cookie")
//!     .http_only(true)
//!     .same_site(SameSite::Strict)
//!     .secure(true) // Set to true in production
//!     .max_age(7200) // 2 hours
//!     .path("/")
//!     .domain("example.com");
//! ```
//!
//! # Important Notes
//!
//! ## Middleware Ordering
//! The `SessionLayer` must be applied **before** the `CookieManagerLayer`:
//!
//! ```rust,no_run
//! use axum::Router;
//! use ruts::{SessionLayer, store::memory::MemoryStore};
//! use tower_cookies::CookieManagerLayer;
//! use std::sync::Arc;
//!
//! let app: Router<()> = Router::new();
//! let session_layer = SessionLayer::new(Arc::new(MemoryStore::new()));
//!
//! // Correct order
//! let router = app
//!     .layer(session_layer)
//!     .layer(CookieManagerLayer::new());
//! ```
//!
//! ## Best Practices
//!
//! - Enable HTTPS in production and set `secure: true` in cookie options.
//! - Use appropriate `SameSite` cookie settings (e.g., `Strict` or `Lax`).
//! - Always set a session expiration time (`max_age`).
//! - Regularly regenerate session IDs using `session.regenerate()` or `session.prepare_regenerate()`,
//!   especially after a change in privilege level (like logging in).
//! - Enable HTTP Only mode (`http_only: true`) to prevent client-side script access to the
//!   session cookie.

pub use cookie;

#[cfg(feature = "axum")]
mod extract;

#[cfg(feature = "redis-store")]
pub use fred;

#[cfg(feature = "postgres-store")]
pub use sqlx;

mod service;
pub use service::*;

mod session;
pub use session::*;

pub mod store;

pub use tower_cookies;
