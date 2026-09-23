# Ruts: Rust Tower Session for HTTP Applications

[![Documentation](https://docs.rs/ruts/badge.svg)](https://docs.rs/ruts)
[![Crates.io](https://img.shields.io/crates/v/ruts.svg)](https://crates.io/crates/ruts)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-1.85.0%2B-blue.svg?maxAge=3600)](https://github.com/jimmielovell/ruts)

`ruts` is a flexible session management middleware for Rust's Tower web
framework.

## Quick Start

Here's a basic example with [Axum](https://docs.rs/axum/latest/axum/) and the `RedisStore`.
This requires the `axum` (enabled by default) and `redis-store` features.

```rust,no_run
# #[cfg(all(feature = "redis-store", feature = "axum"))]
# mod docs {
use axum::{Router, routing::get};
use ruts::{Session, SessionLayer, CookieOptions};
use ruts::store::{redis::RedisStore, Ttl};
use fred::clients::Client;
use std::sync::Arc;
use fred::interfaces::ClientLike;
use tower_cookies::CookieManagerLayer;

#[tokio::main]
async fn main() {
    // Set up Redis client
    let client = Client::default();
    client.connect();
    client.wait_for_connect().await.unwrap();

    // Create session store
    let store = RedisStore::new(Arc::new(client)).await.unwrap();

    // Configure session-cookie options
    let cookie_options = CookieOptions::build()
        .name("session")
        .http_only(true)
        .same_site(cookie::SameSite::Lax)
        .secure(true)
        .max_age(3600) // 1 hour
        .path("/");

    // Create session layer
    let session_layer = SessionLayer::new(Arc::new(store))
        .with_cookie_options(cookie_options);

    // Set up router with session management
    let app = Router::new()
        .route("/", get(handler))
        .layer(session_layer)
        .layer(CookieManagerLayer::new()); // CookieManagerLayer must be after

    // Run the server
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn handler(session: Session<RedisStore<Client>>) -> String {
    let count: Option<i32> = session.get("count").await.unwrap();
    let new_count = count.unwrap_or(0) + 1;
    session.set("count", &new_count, Ttl::new(3600).unwrap(), None).await.unwrap();
    format!("You've visited this page {} times", new_count)
}
# }
# fn main() {}
```

## Session Management

### Basic Operations

```rust,no_run
# #[cfg(feature = "moka-store")]
# mod docs {
use ruts::Session;
use ruts::store::{SessionMap, Ttl};
use ruts::store::moka::MokaStore;

#[derive(serde::Deserialize)]
struct User;

async fn handler(session: Session<MokaStore>) {
    let hour = Ttl::new(3600).unwrap();

    // Get a single field's data
    let value: Option<User> = session.get("key").await.unwrap();

    // Get all session data as a map for lazy deserialization
    let session_map: Option<SessionMap> = session.get_all().await.unwrap();
    if let Some(map) = session_map {
        let user: Option<User> = map.get("user").unwrap();
    }

    // Insert or overwrite a field, with its own TTL
    session.set("key", &"new_value", hour, None).await.unwrap();

    // Give a field a new lease of life, reporting whether it was still live
    let extended: bool = session.expire_field("key", hour).await.unwrap();

    // Prepare a new session id before a write, to prevent session fixation
    let new_id = session.prepare_regenerate();
    session.set("key", &"value_with_new_id", hour, None).await.unwrap();

    // Remove a single field, reporting whether it was there to remove
    let removed: bool = session.remove("key").await.unwrap();

    // Delete the entire session
    session.delete().await.unwrap();

    // Regenerate the session id immediately
    session.regenerate().await.unwrap();

    // Change the cookie's lifetime for this response
    session.set_expiration(7200);

    // Get the current session id
    let id = session.id();
}
# }
# fn main() {}
```

### Field TTLs

Every write takes a `Ttl`, a validated `0..=i32::MAX` number of seconds. There
is no "no expiry": a field always has a horizon, and the session lives for as
long as its longest-lived field.

A `Ttl` of zero means *do not store this*:

```rust,no_run
# #[cfg(feature = "moka-store")]
# mod docs {
use ruts::Session;
use ruts::store::Ttl;
use ruts::store::moka::MokaStore;

async fn handler(session: Session<MokaStore>) {
    // Writing with a zero TTL removes the field instead of persisting it.
    session.set("hero", &"Thor", Ttl::ZERO, None).await.unwrap();

    // Expiring with a zero TTL removes the field rather than extending it.
    let removed: bool = session.expire_field("hero", Ttl::ZERO).await.unwrap();
}
# }
# fn main() {}
```

## Stores

`ruts` offers several backend stores for session data, each behind a feature flag.

| Feature | Store |
| --- | --- |
| `redis-store` | `RedisStore` |
| `postgres-store` | `PostgresStore` |
| `scylla-store` | `ScyllaStore` |
| `moka-store` | `MokaStore` |
| `layered-store` | `LayeredStore` |

`layered-store` does not pull in any backend of its own, so you can configure the combination you want; for example `features = ["layered-store", "redis-store", "scylla-store"]`.

### Redis

A high-performance Redis-backed session store. Ideal for production use as a primary
or caching layer.

#### Requirements

- The `redis-store` feature.
- Redis 7.4 or later (required for field-level expiration using [`HEXPIRE`](https://redis.io/docs/latest/commands/hexpire/)).

```rust,no_run
# #[cfg(feature = "redis-store")]
# mod docs {
use std::sync::Arc;
use fred::clients::Client;
use fred::interfaces::ClientLike;
use ruts::store::redis::RedisStore;

#[tokio::main]
async fn main() {
    let client = Client::default();
    client.init().await.unwrap();

    // Pre-loads the Lua scripts the store runs.
    let store = RedisStore::new(Arc::new(client)).await.unwrap();
}
# }
# fn main() {}
```

### Postgres

A durable, persistent session store backed by a Postgres database.

#### Requirements

- The `postgres-store` feature.

```rust,no_run
# #[cfg(feature = "postgres-store")]
# mod docs {
use sqlx::PgPool;
use ruts::store::postgres::PostgresStoreBuilder;

#[tokio::main]
async fn main() {
    // Set up your database connection pool.
    let database_url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must be set");
    let pool = PgPool::connect(&database_url).await.unwrap();

    // Create the session store using the builder.
    let store = PostgresStoreBuilder::new(pool)
        // Creates the sessions table if it is missing.
        .create_table(true)
        // Optionally, you can customize the schema and table name
        // .schema_name("my_app").unwrap()
        // .table_name("user_sessions").unwrap()
        .build()
        .await
        .unwrap();
}
# }
# fn main() {}
```

Expired rows are swept by a background task; its interval is configurable with
`cleanup_interval`.

### ScyllaDB

A durable session store backed by ScyllaDB (or Cassandra), for deployments that
need sessions spread across many nodes.

#### Requirements

- The `scylla-store` feature.

The session id the client presents is not the partition key: the store keeps a
small mapping row from the presented id to an internal id, so rotating a session
id re-points that row instead of copying every field.

```rust,no_run
# #[cfg(feature = "scylla-store")]
# mod docs {
use std::sync::Arc;
use scylla::client::session_builder::SessionBuilder;
use ruts::store::scylla::ScyllaStoreBuilder;

#[tokio::main]
async fn main() {
    let session = SessionBuilder::new()
        .known_node("127.0.0.1:9042")
        .build()
        .await
        .unwrap();

    let store = ScyllaStoreBuilder::new(Arc::new(session))
        .keyspace_name("my_app")
        .unwrap()
        .table_name("sessions")
        .unwrap()
        // Creates the keyspace and tables if they are missing.
        .create_table(true)
        .build()
        .await
        .unwrap();
}
# }
# fn main() {}
```

### Moka

An in-process store backed by [`moka`](https://crates.io/crates/moka). Useful for
tests, single-node deployments, and as the hot tier of a `LayeredStore`.

#### Requirements

- The `moka-store` feature.

```rust,no_run
# #[cfg(feature = "moka-store")]
# fn docs() {
use ruts::store::moka::MokaStoreBuilder;

let store = MokaStoreBuilder::new()
    .max_capacity(10_000)
    .build();
# }
# fn main() {}
```

### LayeredStore

A composite store that layers a fast, ephemeral "hot" cache (like Redis) on top of a
slower, persistent "cold" store (like Postgres or Scylla). It is designed for scenarios
where sessions can have long lifespans but should only occupy expensive cache memory
when actively being used, thus balancing performance and durability.

Reads try the hot store first; a miss falls through to the cold store and warms the
cache with what it finds. Writes go to both.

**Note**: Requires the `layered-store` feature alongside your chosen combination of hot
and cold backend stores (e.g., `redis-store` and `postgres-store`, or `redis-store`
and `scylla-store`).

```rust,no_run
# #[cfg(all(feature = "layered-store", feature = "redis-store", feature = "postgres-store"))]
# mod docs {
use ruts::store::redis::RedisStore;
use ruts::store::postgres::PostgresStore;
use fred::clients::Client;
use ruts::store::layered::LayeredStore;
use ruts::Session;

use ruts::store::Ttl;

// Define a type alias for your specific layered store setup
type MyLayeredStore = LayeredStore<RedisStore<Client>, PostgresStore>;
type MySession = Session<MyLayeredStore>;

#[derive(serde::Serialize)]
struct User { id: i32 }

async fn handler(session: MySession) {
    let user = User { id: 1 };

    // This session field is valid for 1 month in the persistent store.
    let long_term_expiry = Ttl::new(60 * 60 * 24 * 30).unwrap();

    // However, we only want it to live in the hot cache (Redis) for 1 hour.
    let short_term_hot_cache_expiry = Ttl::new(60 * 60).unwrap();

    // The cold store (Postgres) will get the long-term expiry,
    // but the hot store (Redis) will be capped at the shorter TTL.
    session.set("user", &user, long_term_expiry, Some(short_term_hot_cache_expiry))
        .await
        .unwrap();

    // A hot-cache TTL of zero keeps a field out of the hot store entirely: it is
    // persisted in the cold store and read from there, but never cached in the
    // hot store.
    session.set("idempotency-key", &user, long_term_expiry, Some(Ttl::ZERO))
        .await
        .unwrap();
}
# }
# fn main() {}
```

## Serialization

Ruts supports two serialization backends for session data storage:

- [`bincode`](https://crates.io/crates/bincode) (default) - Fast, compact binary serialization.
- [`rmp-serde`](https://crates.io/crates/rmp-serde) (MessagePack) - Cross-language
  compatible serialization.

To use `MessagePack` instead of the default `bincode`, add this to your `Cargo.toml`:

```toml
[dependencies]
ruts = { version = "0.11", default-features = false, features = ["axum", "messagepack"] }
```

Cargo features are additive, so enabling `messagepack` without turning the
defaults off leaves `bincode` enabled too. `messagepack` takes precedence and
`bincode` is unused.

The two formats are not wire-compatible. Switching backends on a store that
already holds sessions invalidates every field in it.

## Cookie Configuration

```rust
use ruts::CookieOptions;
use ruts::cookie::SameSite;

let cookie_options = CookieOptions::build()
    .name("my_session_cookie")
    .http_only(true)
    .same_site(SameSite::Strict)
    .secure(true) // Set to true in production
    .max_age(7200) // 2 hours
    .path("/")
    .domain("example.com");
```

### Signed Cookies

Ruts supports cryptographically signed cookies to prevent client-side
tampering of the session id. To use this, you must enable the `signed`
feature in your `Cargo.toml`:

```toml
[dependencies]
ruts = { version = "0.11", features = ["signed"] }
```

Then you can provide a `ruts::Key` (A re-export of `tower_cookies::Key`)
to your CookieOptions.

```rust
# #[cfg(feature = "signed")]
# fn main() {
use ruts::CookieOptions;
use cookie::SameSite;
use tower_cookies::Key;

let key = Key::generate();
let cookie_options = CookieOptions::build()
    .name("secure_session")
    .http_only(true)
    .same_site(SameSite::Lax)
    .secure(true)
    .max_age(3600)
    .path("/")
    .signing_key(key);
# }
# #[cfg(not(feature = "signed"))]
# fn main() {}
```

## Important Notes

### Middleware Ordering

The `SessionLayer` must be applied **before** the `CookieManagerLayer`:

```rust,no_run
# #[cfg(all(feature = "axum", feature = "moka-store"))]
# fn main() {
use axum::Router;
use ruts::{SessionLayer, store::moka::MokaStoreBuilder};
use tower_cookies::CookieManagerLayer;
use std::sync::Arc;

let app: Router<()> = Router::new();
let session_layer = SessionLayer::new(Arc::new(MokaStoreBuilder::new().build()));

// Correct order
let router = app
    .layer(session_layer)
    .layer(CookieManagerLayer::new());
# }
# #[cfg(not(all(feature = "axum", feature = "moka-store")))]
# fn main() {}
```

### Best Practices

- Enable HTTPS in production and set `secure: true` in cookie options.
- Use appropriate `SameSite` cookie settings (e.g., `Strict` or `Lax`).
- Always set a session expiration time (`max_age`).
- Regularly regenerate session ids using `session.regenerate()` or
  `session.prepare_regenerate()`, especially after a change in privilege level
  (like logging in).
- Enable HTTP Only mode (`http_only: true`) to prevent client-side script access to the
  session cookie.

## Contributing

Contributions are welcome! Please feel free to submit issues and pull requests.

## License

This project is licensed under the MIT License - see the
[LICENSE](https://github.com/jimmielovell/ruts/blob/main/LICENSE) file for details.