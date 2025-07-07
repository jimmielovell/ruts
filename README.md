# Ruts: Rust Tower Session for HTTP Applications

[![Documentation](https://docs.rs/ruts/badge.svg)](https://docs.rs/ruts)
[![Crates.io](https://img.shields.io/crates/v/ruts.svg)](https://crates.io/crates/ruts)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-1.75.0%2B-blue.svg?maxAge=3600)](
github.com/jimmielovell/ruts)

Ruts is a robust, flexible session management library for Rust web applications. It provides a seamless way to handle cookie sessions in tower-based web frameworks, with a focus on security, performance, and ease of use.

## Features

- 🚀 High-performance session management
- 🔒 Secure by default with configurable options
- 🔄 Built-in Redis session store support
- 🛠 Flexible API supporting custom session stores
- ⚡ Optimized for tower-based frameworks like axum
- 🍪 Comprehensive cookie management

## Installation

Add the following to your `Cargo.toml`:

```toml
[dependencies]
ruts = "0.5.8"
```

## Quick Start

Here's a basic example using `ruts` with [axum](https://docs.rs/axum/latest/axum/):

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
        .layer(session_layer)             // SessionLayer must be below
        .layer(CookieManagerLayer::new()); // CookieManagerLayer must be on top

    // Run the server
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn handler(session: Session<RedisStore<Client>>) -> String {
    let count: Option<i32> = session.get("count").await.unwrap();
    let new_count = count.unwrap_or(0) + 1;
    session.insert("count", &new_count).await.unwrap();
    format!("You've visited this page {} times", new_count)
}
```

## Session Management

### Basic Operations

```rust
// Get session data
let value: ValueType = session.get("key").await?;

// Insert new data
session.insert("key", &value, optional_field_expiration).await?;

// Prepare a new session ID for the next insert
let new_id = session.prepare_regenerate();
session.insert("key", &value, optional_field_expiration).await?;

// Update existing data
session.update("key", &new_value, optional_field_expiration).await?;

// Prepare a new session ID for the next update
let new_id = session.prepare_regenerate();
session.update("key", &value, optional_field_expiration).await?;

// Remove data
session.remove("key").await?;

// Delete entire session
session.delete().await?;

// Regenerate session ID (for security)
session.regenerate().await?;

// Update session expiry
session.expire(seconds)

// Get session ID
session.id()
```

### Redis Store (Default session store)
A Redis-backed session store implementation.

#### Requirements

- Redis 7.4 or later (required for field-level expiration using HEXPIRE)
- For Redis < 7.4, field-level expiration will not be available

```rust
use ruts::store::redis::RedisStore;

let store = RedisStore::new(Arc::new(fred_client_or_pool));
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
    .domain("example.com");  // Optional
```

## Important Notes

### Middleware Ordering
When using cookie-based sessions, the SessionLayer must be applied **before** the CookieManagerLayer:

```rust
app.layer(session_layer)              // First: Session layer
   .layer(CookieManagerLayer::new()); // Then: Cookie layer on top
```

### Security Best Practices

- Enable HTTPS in production (set `secure: true` in cookie options)
- Use appropriate `SameSite` cookie settings
- Add session expiration
- Regularly regenerate session IDs
- Set proper cookie attributes (`http_only: true`)

## Error Handling

The library provides a comprehensive error type for handling various session-related errors:

```rust
match session.get::<User>("user").await {
    Ok(Some(user)) => { /* Handle user */ }
    Ok(None) => { /* No user found */ }
    Err(e) => { /* Handle error */ }
}
```

## Contributing

Contributions are welcome! Please feel free to submit issues and pull requests.

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.
