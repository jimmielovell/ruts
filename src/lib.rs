//! # Ruts: Rust Tower Session for HTTP Applications
//!
//! ## Quick Start
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
//!     session.insert("count", &new_count, None).await.unwrap();
//!     format!("You've visited this page {} times", new_count)
//! }
//! ```
//!
//! ## Serialization
//!
//! Ruts supports two serialization backends for session data storage:
//!
//! - `bincode` (default) - Fast binary serialization
//! - `messagepack` - Cross-language compatible serialization
//!
//! To use `MessagePack` instead of the default `bincode`:
//!
//! ```toml
//! [dependencies]
//! ruts = { version = "0.5.9", default-features = false, features = ["axum", "redis-store", "messagepack"] }
//! ```
//!
//! ## Important Notes
//!
//! ### Middleware Ordering
//!
//! The `SessionLayer` must be applied **before** the `CookieManagerLayer`:
//!
//! ### Redis Requirements
//!
//! - Redis 7.4 or later (required for field-level expiration using [HEXPIRE](https://redis.io/docs/latest/commands/hexpire/))
//! - For Redis < 7.4, field-level expiration will not be available

pub use cookie;

#[cfg(feature = "axum")]
mod extract;

#[cfg(feature = "redis-store")]
pub use fred;

mod service;
pub use service::*;

mod session;
pub use session::*;

pub mod store;

pub use tower_cookies;