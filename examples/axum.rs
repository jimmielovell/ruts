use axum::routing::get;
use axum::{Json, Router};
use fred::clients::Client;
use fred::interfaces::ClientLike;
use ruts::store::redis::RedisStore;
use ruts::{CookieOptions, Session, SessionLayer};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use tower_cookies::CookieManagerLayer;

type RedisSession = Session<RedisStore<Client>>;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct User {
    id: i64,
    name: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
enum Theme {
    Light,
    #[default]
    Dark,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct AppSession {
    user: Option<User>,
    ip: Option<IpAddr>,
    theme: Option<Theme>,
}

fn routes() -> Router {
    Router::new()
        .route(
            "/insert",
            get(|session: RedisSession| async move {
                let app_session: AppSession = AppSession {
                    user: Some(User {
                        id: 34895634,
                        name: String::from("John Doe"),
                    }),
                    ip: Some(IpAddr::from(Ipv4Addr::new(192, 168, 0, 1))),
                    theme: Some(Theme::Dark),
                };

                session
                    .insert("app", &app_session, None)
                    .await
                    .map_err(|e| e.to_string())
                    .unwrap();
            }),
        )
        .route(
            "/update",
            get(|session: RedisSession| async move {
                session
                    .update("theme", &Theme::Light, None)
                    .await
                    .map_err(|e| e.to_string())
                    .unwrap();
            }),
        )
        .route(
            "/get",
            get(|session: RedisSession| async move {
                let user: Option<AppSession> =
                    session.get("app").await.map_err(|e| e.to_string()).unwrap();
                Json(user.unwrap())
            }),
        )
        .route(
            "/remove",
            get(|session: RedisSession| async move {
                session
                    .remove("app-2")
                    .await
                    .map_err(|e| e.to_string())
                    .unwrap();
            }),
        )
        .route(
            "/regenerate",
            get(|session: RedisSession| async move {
                session
                    .regenerate()
                    .await
                    .map_err(|e| e.to_string())
                    .unwrap();
            }),
        )
        .route(
            "/delete",
            get(|session: RedisSession| async move {
                session.delete().await.map_err(|e| e.to_string()).unwrap();
            }),
        )
}

#[tokio::main]
async fn main() {
    // Set up Redis client
    let client = Client::default();
    client.init().await.unwrap();

    // Create session store
    let store = RedisStore::new(Arc::new(client));

    // Configure session options
    let cookie_options = CookieOptions::build()
        .name("session")
        .http_only(true)
        .same_site(cookie::SameSite::Lax)
        .secure(true)
        .max_age(1 * 60)
        .path("/");

    // Create session layer
    let session_layer = SessionLayer::new(Arc::new(store)).with_cookie_options(cookie_options);

    // Set up router with session management
    let app = routes()
        .layer(session_layer)
        .layer(CookieManagerLayer::new());

    // Run the server
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
