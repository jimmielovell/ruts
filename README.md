# Ruts: Rust Tower Session for HTTP Applications

[![Documentation](https://docs.rs/ruts/badge.svg)](https://docs.rs/ruts)
[![Crates.io](https://img.shields.io/crates/v/ruts.svg)](https://crates.io/crates/ruts)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-1.85.0%2B-blue.svg?maxAge=3600)](
https://github.com/jimmielovell/ruts)

## Quick Start

Here's a basic example with [Axum](https://docs.rs/axum/latest/axum/) and the `RedisStore`. This requires the features `axum`(enabled by default) and `redis-store` to be enabled.

```rust
use axum::{Router, routing::get};
use ruts::{Session, SessionLayer, CookieOptions};
use ruts::store::redis::RedisStore;
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
    let store = RedisStore::new(Arc::new(client));

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
    session.insert("count", &new_count, None).await.unwrap();
    format!("You've visited this page {} times", new_count)
}
```

## Session Management

### Basic Operations

```rust
use ruts::Session;
use ruts::store::SessionMap;
use ruts::store::memory::MemoryStore;

[derive(serde::Deserialize)]
struct User;

async fn handler(session: Session<MemoryStore>) {
  // Get a single field's data
  let value: Option<User> = session.get("key").await.unwrap();

  // Get all session data as a map for lazy deserialization
  let session_map: Option<SessionMap> = session.get_all().await.unwrap();
  if let Some(map) = session_map {
      let user: Option<User> = map.get("user").unwrap();
  }

  // Set data (upsert) with an optional field-level expiration (in seconds)
  // If the field exists, it is overwritten.
  session.set("key", &"some_value", Some(3600), None).await.unwrap();
  
  // Prepare a new session ID before a set operation to prevent session fixation
  let new_id = session.prepare_regenerate();
  // The next set operation will automatically rename the session to the new ID
  session.set("key", &"value_with_new_id", None, None).await.unwrap();
  
  // Remove a single field
  session.remove("key").await.unwrap();
  
  // Delete the entire session
  session.delete().await.unwrap();
  
  // Regenerate session ID for security (Renames session immediately)
  session.regenerate().await.unwrap();
  
  // Update the session's overall expiry time
  session.expire(7200).await.unwrap();
  
  // Get the current session ID
  let id = session.id();
}
```

## Stores

### Redis
A Redis-backed session store.

#### Requirements

- The `redis-store` feature.
- Redis 7.4 or later (required for field-level expiration using [HEXPIRE](https://redis.io/docs/latest/commands/hexpire/))
- For Redis < 7.4, field-level expiration will not be available

```rust
use ruts::store::redis::RedisStore;

let store = RedisStore::new(Arc::new(fred_client_or_pool));
```

### Postgres
A Postgres-backed session store implementation.

#### Requirements

- The `postgres-store` feature.

```rust
use ruts::store::postgres::PostgresStoreBuilder;

// 1. Set up your database connection pool.
let database_url = std::env::var("DATABASE_URL")
    .expect("DATABASE_URL must be set");
let pool = PgPool::connect(&database_url).await.unwrap();

// 2. Create the session store using the builder.
// This will also run a migration to create the `sessions` table.
let store = PostgresStoreBuilder::new(pool, true)
    // Optionally, you can customize the schema and table name
    // .schema_name("my_app")
    // .table_name("user_sessions")
    .build()
    .await
    .unwrap();
```

### LayeredStore

A composite store that layers a fast, ephemeral "hot" cache (like Redis) on top of a slower, persistent "cold" store (like Postgres). It is designed for scenarios where sessions can have long lifespans but should only occupy expensive cache memory when actively being used thus balancing performance and durability.

```rust
use ruts::store::layered::LayeredWriteStrategy;
use ruts::Session;
use ruts::store::redis::RedisStore;
use ruts::store::postgres::PostgresStore;
use ruts::store::layered::LayeredStore;

type MySession = Session<LayeredStore<RedisStore, PostgresStore>>;

#[derive(serde::Serialize)]
struct User { id: i32 }

async fn handler(session: MySession) {
    let user = User { id: 1 };
    
    // This session field is valid for 1 month in the persistent store.
    let long_term_expiry = 60  * 60 * 24 * 30;
    
    // However, we only want it to live in the hot cache (Redis) for 1 hour.
    let short_term_hot_cache_expiry = 60 * 60;
    
    // The cold store (Postgres) will get the long-term expiry,
    // but the hot store (Redis) will be capped at the shorter TTL.
    session.update("user", &user, Some(long_term_expiry), Some(short_term_hot_cache_expiry)).await.unwrap();
}
```

### Serialization
Ruts supports two serialization backends for session data storage:

- [`bincode`](https://crates.io/crates/bincode) (default) - Fast, compact binary serialization.
- [`messagepack`](https://crates.io/crates/rmp-serde) - Cross-language compatible serialization

To use [`MessagePack`](https://crates.io/crates/rmp-serde) instead of the default [`bincode`](https://crates.io/crates/bincode), add this to your `Cargo.toml`:

```toml
[dependencies]
ruts = { version = "0.7.2", default-features = false, features = ["axum", "messagepack"] }
```

### Cookie Configuration

```rust
let cookie_options = CookieOptions::build()
    .name("cookie_name")
    .http_only(true)
    .same_site(cookie::SameSite::Strict)
    .secure(true)
    .max_age(7200) // 2 hours
    .path("/")
    .domain("example.com");
```

## Important Notes

### Middleware Ordering
The `SessionLayer` must be applied **before** the `CookieManagerLayer`:

```rust
app.layer(session_layer)              // First: SessionLayer
   .layer(CookieManagerLayer::new()); // Then CookieManagerLayer
```

### Best Practices

- Enable HTTPS in production (set `secure: true` in cookie options)
- Use appropriate `SameSite` cookie settings
- Add session expiration
- Regularly regenerate session IDs
- Enable HTTP Only mode in production (set `http_only: true`)

## Contributing

Contributions are welcome! Please feel free to submit issues and pull requests.

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.
