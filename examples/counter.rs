use axum::{Router, routing::get};
use ruse::{Session, SessionLayer, CookieOptions};
use ruse::store::redis::RedisStore;
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
