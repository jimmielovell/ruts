use std::net::Ipv4Addr;
use std::{net::IpAddr, sync::Arc};

use axum::{routing::get, Json, Router};
use tower_cookies::CookieManagerLayer;

use fred::types::InfoKind::Default;
use fred::{clients::RedisClient, error::RedisError, interfaces::ClientLike};
use ruse::{CookieOptions, RedisStore, Session, SessionLayer};
use serde::{Deserialize, Serialize};
use tower::ServiceExt;

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

#[tokio::main]
async fn main() -> Result<(), RedisError> {
    let cookie_options = CookieOptions::build()
        .name("wsess")
        .http_only(true)
        .same_site(cookie::SameSite::Lax)
        .secure(false)
        .max_age(10 * 60);

    let client = RedisClient::default();
    client.init().await?;

    let store = RedisStore::new(Arc::new(client));
    let session_layer = SessionLayer::new(Arc::new(store)).with_cookie_options(cookie_options);

    let app = Router::new()
        .route("/", get(set_session))
        .route("/add", get(add_session))
        .route("/session", get(get_session))
        .route("/remove", get(remove_session_key))
        // .route("/regenerate", get(regenerate_session))
        .route("/delete", get(delete_session))
        .layer(session_layer)
        .layer(CookieManagerLayer::new());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();

    Ok(())
}

async fn set_session(session: Session) -> Result<(), String> {
    let app = AppSession {
        user: Some(User {
            id: 33826744,
            name: String::from("Jimmie Lovell"),
        }),
        ip: Some(IpAddr::from(Ipv4Addr::new(127, 0, 0, 1))),
        theme: Some(Theme::Dark),
    };

    session
        .insert("app", app)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

async fn get_session(session: Session) -> Result<Json<AppSession>, String> {
    let user = session.get("app").await.map_err(|e| e.to_string())?;
    Ok(Json(user.unwrap()))
}

async fn add_session(session: Session) -> Result<(), String> {
    let app = AppSession {
        user: Some(User {
            id: 37325314,
            name: String::from("Jenipher Mawia"),
        }),
        ip: Some(IpAddr::from(Ipv4Addr::new(127, 0, 1, 1))),
        theme: Some(Theme::Light),
    };

    session
        .insert("app-2", app)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

async fn remove_session_key(session: Session) -> Result<(), String> {
    session
        .remove("app-2")
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

async fn delete_session(session: Session) -> Result<(), String> {
    session
        .delete()
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

async fn regenerate_session(session: Session) -> Result<(), String> {
    session
        .regenerate()
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}
