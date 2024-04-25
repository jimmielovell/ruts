use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use axum::{Json, Router};
use axum::routing::get;
use fred::clients::{RedisClient};
use fred::interfaces::ClientLike;
use serde::{Deserialize, Serialize};
use tower_cookies::CookieManagerLayer;
use ruse::{CookieOptions, Session, SessionLayer};
use ruse::store::redis::RedisStore;

type RedisSession = Session<RedisStore<RedisClient>>;

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
            "/set",
            get(|session: RedisSession| async move {
                let app = AppSession {
                    user: Some(User {
                        id: 34895634,
                        name: String::from("John Doe"),
                    }),
                    ip: Some(IpAddr::from(Ipv4Addr::new(192, 168, 0, 1))),
                    theme: Some(Theme::Dark),
                };

                session
                    .insert("app", app)
                    .await
                    .map_err(|e| e.to_string())
                    .unwrap();
            }),
        )
        .route(
            "/add",
            get(|session: RedisSession| async move {
                let app = AppSession {
                    user: Some(User {
                        id: 12432312,
                        name: String::from("Jane Doe"),
                    }),
                    ip: Some(IpAddr::from(Ipv4Addr::new(192, 168, 1, 1))),
                    theme: Some(Theme::Light),
                };

                session
                    .insert("app-2", app)
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
        // .route("/regenerate", get(regenerate_session))
        .route(
            "/delete",
            get(|session: RedisSession| async move {
                session.delete().await.map_err(|e| e.to_string()).unwrap();
            }),
        )
}

#[tokio::main]
async fn main() {
    let cookie_options = CookieOptions::build()
        .name("test_sess")
        .http_only(true)
        .same_site(cookie::SameSite::Lax)
        .secure(true)
        .max_age(1 * 60)
        .path("/");

    let client = RedisClient::default();
    client.init().await.unwrap();

    // let pool = Builder::default_centralized().build_pool(2).unwrap();
    // pool.init().await.unwrap();

    let store = RedisStore::new(Arc::new(client));
    let session_layer = SessionLayer::new(Arc::new(store)).with_cookie_options(cookie_options);

    let app = routes()
        .layer(session_layer)
        .layer(CookieManagerLayer::new());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
