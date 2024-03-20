use axum::{routing::get, Json, Router};
use axum_core::body::Body;
use cookie::time::Duration;
use cookie::SameSite;
use fred::{clients::RedisClient, interfaces::ClientLike};
use http::{header, HeaderMap, Request, StatusCode};
use http_body_util::BodyExt;
use ruse::{CookieOptions, Session, SessionLayer, store::redis::RedisStore};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use axum_core::response::Response;
use tower::ServiceExt;
use tower_cookies::{cookie, Cookie, CookieManagerLayer};

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
                println!("here");
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
                    .map_err(|e| e.to_string())
                    .unwrap();
            }),
        )
        .route(
            "/add",
            get(|session: RedisSession| async move {
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

pub async fn app() -> Router {
    let cookie_options = CookieOptions::build()
        .name("test_sess")
        .http_only(true)
        .same_site(cookie::SameSite::Lax)
        .secure(true)
        .max_age(1 * 60)
        .path("/");

    let client = RedisClient::default();
    client.init().await.unwrap();

    let store = RedisStore::new(Arc::new(client));
    let session_layer = SessionLayer::new(Arc::new(store)).with_cookie_options(cookie_options);

    routes()
        .layer(session_layer)
        .layer(CookieManagerLayer::new())

    // let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    // axum::serve(listener, app).await.unwrap();
}

pub async fn body_string(body: Body) -> String {
    let bytes = body.collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).into()
}

pub async fn set_session_req() -> Response {
    let req = Request::builder()
        .uri("/set")
        .body(Body::empty())
        .unwrap();
    app().await.oneshot(req).await.unwrap()
}

pub async fn get_session_req(cookie_val: &str) -> Response {
    let cookie = Cookie::new("test_sess", cookie_val);
    let req = Request::builder()
        .uri("/get")
        .header(header::COOKIE, cookie.encoded().to_string())
        .body(Body::empty())
        .unwrap();
    app().await.oneshot(req).await.unwrap()
}

pub fn get_session_cookie(headers: &HeaderMap) -> Result<Cookie<'_>, cookie::ParseError> {
    headers
        .get_all(header::SET_COOKIE)
        .iter()
        .flat_map(|header| header.to_str())
        .next()
        .ok_or(cookie::ParseError::MissingPair)
        .and_then(Cookie::parse_encoded)
}

#[tokio::test]
async fn no_session_set() {
    let req = Request::builder().uri("/").body(Body::empty()).unwrap();
    let res = app().await.oneshot(req).await.unwrap();

    assert!(res
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .next()
        .is_none());
}

#[tokio::test]
async fn bogus_session_cookie() {
    let session_cookie = Cookie::new("id", "00000000-0000-0000-0000-000000000000");
    let req = Request::builder()
        .uri("/set")
        .header(header::COOKIE, session_cookie.encoded().to_string())
        .body(Body::empty())
        .unwrap();
    let res = app().await.oneshot(req).await.unwrap();
    let session_cookie = get_session_cookie(res.headers()).unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    assert_ne!(
        session_cookie.value(),
        "00000000-0000-0000-0000-000000000000"
    );
}

#[tokio::test]
async fn malformed_session_cookie() {
    set_session_req().await;

    let res = get_session_req("malformed").await;

    println!("{:?}", res);

    let session_cookie = get_session_cookie(res.headers()).unwrap();
    assert_ne!(session_cookie.value(), "malformed");
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn set_session() {
    let res = set_session_req().await;
    let session_cookie = get_session_cookie(res.headers()).unwrap();

    assert_eq!(session_cookie.name(), "test_sess");
    assert_eq!(session_cookie.http_only(), Some(true));
    assert_eq!(session_cookie.same_site(), Some(SameSite::Lax));
    assert!(session_cookie
        .max_age()
        .is_some_and(|dt| dt <= Duration::minutes(1)));
    assert_eq!(session_cookie.secure(), Some(true));
    assert_eq!(session_cookie.path(), Some("/"));
}

#[tokio::test]
async fn session_max_age() {
    let req = Request::builder().uri("/set").body(Body::empty()).unwrap();
    let res = app().await.oneshot(req).await.unwrap();
    let session_cookie = get_session_cookie(res.headers()).unwrap();

    assert_eq!(session_cookie.name(), "test_sess");
    assert_eq!(session_cookie.http_only(), Some(true));
    assert_eq!(session_cookie.same_site(), Some(SameSite::Strict));
    assert!(session_cookie
        .max_age()
        .is_some_and(|d| d <= Duration::weeks(2)));
    assert_eq!(session_cookie.secure(), Some(true));
    assert_eq!(session_cookie.path(), Some("/"));
}

#[tokio::test]
async fn get_session() {
    let app = app().await;

    let req = Request::builder().uri("/set").body(Body::empty()).unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let session_cookie = get_session_cookie(res.headers()).unwrap();

    let req = Request::builder()
        .uri("/get")
        .header(header::COOKIE, session_cookie.encoded().to_string())
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_string(res.into_body()).await, "{\"user\":{\"id\":33826744,\"name\":\"Jimmie Lovell\"},\"ip\":\"127.0.0.1\",\"theme\":\"Dark\"}");
}

#[tokio::test]
async fn get_no_value() {
    let app = app().await;

    let req = Request::builder().uri("/get").body(Body::empty()).unwrap();
    let res = app.oneshot(req).await.unwrap();

    assert_eq!(body_string(res.into_body()).await, "None");
}

#[tokio::test]
async fn remove_field() {
    let app = app().await;

    let req = Request::builder()
        .uri("/set")
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let session_cookie = get_session_cookie(res.headers()).unwrap();

    let req = Request::builder()
        .uri("/remove")
        .header(header::COOKIE, session_cookie.encoded().to_string())
        .body(Body::empty())
        .unwrap();
    app.clone().oneshot(req).await.unwrap();

    let req = Request::builder()
        .uri("/get_value")
        .header(header::COOKIE, session_cookie.encoded().to_string())
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();

    assert_eq!(body_string(res.into_body()).await, "None");
}

#[tokio::test]
async fn cycle_session_id() {
    let app = app().await;

    let req = Request::builder()
        .uri("/set")
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let first_session_cookie = get_session_cookie(res.headers()).unwrap();

    let req = Request::builder()
        .uri("/cycle_id")
        .header(header::COOKIE, first_session_cookie.encoded().to_string())
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let second_session_cookie = get_session_cookie(res.headers()).unwrap();

    let req = Request::builder()
        .uri("/get")
        .header(header::COOKIE, second_session_cookie.encoded().to_string())
        .body(Body::empty())
        .unwrap();
    dbg!("foo");
    let res = dbg!(app.oneshot(req).await).unwrap();

    assert_ne!(first_session_cookie.value(), second_session_cookie.value());
    assert_eq!(body_string(res.into_body()).await, "42");
}

#[tokio::test]
async fn flush_session() {
    let app = app().await;

    let req = Request::builder()
        .uri("/set")
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let session_cookie = get_session_cookie(res.headers()).unwrap();

    let req = Request::builder()
        .uri("/flush")
        .header(header::COOKIE, session_cookie.encoded().to_string())
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();

    let session_cookie = get_session_cookie(res.headers()).unwrap();

    assert_eq!(session_cookie.value(), "");
    assert_eq!(session_cookie.max_age(), Some(Duration::ZERO));
}
