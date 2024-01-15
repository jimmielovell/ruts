use axum::{routing::get, Router};
use ruse::{CookieOptions, Session, SessionLayer};
use tower_cookies::CookieManagerLayer;

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/", get(set_cookie))
        .route("/session", get(get_session))
        .layer(SessionLayer::new().with_cookie_options(CookieOptions {
            name: "wsess",
            domain: None,
            path: None,
            secure: false,
            http_only: true,
        }))
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
