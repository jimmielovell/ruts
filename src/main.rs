use axum::{routing::get, Router};
use ruse::{cookie, CookieOptions, MemoryStore, Session, SessionLayer};
use tower_cookies::CookieManagerLayer;

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
        .route("/", get(set_cookie))
        .route("/session", get(get_session))
        .layer(session_layer)
        .layer(CookieManagerLayer::new());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn set_cookie(session: Session) {
    session.insert("cook", "hello_world");
}

async fn get_session(session: Session) -> String {
    String::from("some value")
}
