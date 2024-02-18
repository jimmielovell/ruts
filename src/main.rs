use std::net::IpAddr;

use axum::{routing::get, Router};
use ruse::{cookie, stores::MemoryStore, CookieOptions, Session, SessionLayer};
use tower_cookies::CookieManagerLayer;

#[derive(Clone, Debug, Default)]
struct User {
    id: u64,
    name: String,
}

#[derive(Clone, Debug, Default)]
enum Theme {
    Light,
    #[default]
    Dark,
}

#[derive(Clone, Debug, Default)]
struct MySession {
    user: Option<User>,
    ip: Option<IpAddr>,
    theme: Option<Theme>,
}

#[tokio::main]
async fn main() {
    let cookie_options = CookieOptions::build()
        .name("wsess")
        .http_only(true)
        .same_site(cookie::SameSite::Lax)
        .secure(false);

    let store = MemoryStore::new();
    let session_layer = SessionLayer::new(store).with_cookie_options(cookie_options);

    let app = Router::new()
        .route("/", get(set_session))
        .route("/session", get(get_session))
        .layer(session_layer)
        .layer(CookieManagerLayer::new());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn set_session(session: Session<MySession>) {
    session.insert("user", User::default());
}

async fn get_session(session: Session<MySession>) -> User {
    session.get("user")
}
