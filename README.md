# Ruts: Rust Tower Session for HTTP Applications

Ruts is a robust, flexible session management library for Rust web applications. It provides a seamless way to handle user sessions in tower-based web frameworks, with a focus on security, performance, and ease of use.

## Features

- Compatible with tower-based web frameworks (e.g., axum, warp)
- Flexible session storage with built-in support for Redis
- Customizable cookie options for session management
- Support for both cookie-based and URL-based sessions
- Asynchronous API for efficient session handling
- Type-safe session data management
- Automatic session expiration and regeneration

## Installation

Add the following to your `Cargo.toml`:

```toml
[dependencies]
ruts = "0.1.11"
```

## Quick Start

Here's a basic example of how to use Ruse with axum:

```rust
use axum::{Router, routing::get};
use ruts::{Session, SessionLayer, CookieOptions};
use ruts::store::redis::RedisStore;
use fred::clients::RedisClient;
use std::sync::Arc;
use fred::interfaces::ClientLike;
use tower_cookies::CookieManagerLayer;

#[tokio::main]
async fn main() {
    // Set up Redis client
    let client = RedisClient::default();
    client.init().await.unwrap();

    // Create session store
    let store = RedisStore::new(Arc::new(client));

    // Configure session options
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
        .layer(CookieManagerLayer::new());

    // Run the server
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn handler(session: Session<RedisStore<RedisClient>>) -> String {
    // Use the session in your handler
    let count: i32 = session.get("count").await.map_err(|err| {
        println!("{err:?}");
    }).unwrap().unwrap_or(0);
    session.update("count", count + 1).await.unwrap();
    format!("You've visited this page {} times", count + 1)
}
```

## Usage

### Setting Up the Session Layer

To use Ruse in your application, you need to set up the `SessionLayer`:

```rust
let store = RedisStore::new(Arc::new(redis_client));
let session_layer = SessionLayer::new(Arc::new(store))
    .with_cookie_options(CookieOptions::build().name("session").max_age(3600));
```

### Using Sessions in Request Handlers

Ruse provides an extractor for axum that allows you to easily access the session in your request handlers:

```rust
async fn handler(session: Session<RedisStore<RedisClient>>) -> impl IntoResponse {
    // Use session methods here
}
```

### Session Methods

Ruse provides several methods for working with session data:

- `insert`: Add a new key-value pair to the session
- `get`: Retrieve a value from the session
- `update`: Update an existing value in the session
- `remove`: Remove a key-value pair from the session
- `delete`: Delete the entire session
- `regenerate`: Generate a new session ID
- `expire`: Set or update the session expiration time

## Configuration

You can customize various aspects of session management using `CookieOptions`:

```rust
let cookie_options = CookieOptions::build()
    .name("custom_session")
    .http_only(true)
    .same_site(cookie::SameSite::Strict)
    .secure(true)
    .max_age(7200) // 2 hours
    .path("/app");
```

## Security Considerations

- Always use HTTPS in production to protect session cookies.
- Set appropriate `SameSite` and `Secure` flags for cookies.
- Regularly regenerate session IDs to prevent session fixation attacks.
- Be cautious about what data you store in sessions and how long sessions last.

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.
