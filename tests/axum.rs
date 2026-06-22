mod common;

use crate::common::{TestData, create_test_data};
use axum::{
    Json, Router,
    body::Body,
    http::{Request, StatusCode},
    routing::get,
};
use http::header::{COOKIE, SET_COOKIE};
#[cfg(feature = "signed")]
use ruts::Key;
use ruts::store::moka::{MokaStore, MokaStoreBuilder};
use ruts::store::{SessionStore, Ttl};
use ruts::{CookieOptions, Session, SessionLayer};
use std::sync::Arc;
use tower::ServiceExt;
use tower_cookies::CookieManagerLayer;

const COOKIE_MAX_AGE: u64 = 15;

async fn insert_handler(session: Session<MokaStore>) -> Result<String, StatusCode> {
    let data = create_test_data();

    // Field TTL (60) is deliberately different from the cookie Max-Age (15):
    // the response cookie must reflect the configured Max-Age, not this TTL.
    session
        .set("data", &data, Ttl::new(60).unwrap(), None)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok("Success".to_string())
}

async fn get_handler(session: Session<MokaStore>) -> Result<Json<Option<TestData>>, StatusCode> {
    let data: Option<TestData> = session
        .get("data")
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(data))
}

async fn delete_handler(session: Session<MokaStore>) -> Result<String, StatusCode> {
    session
        .set("data", &create_test_data(), Ttl::new(60).unwrap(), None)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    session
        .delete()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok("deleted".to_string())
}

pub async fn run_session_operations<S: SessionStore>(session: &Session<S>) {
    let test_data = create_test_data();

    session
        .set("test", &test_data, Ttl::new(3600).unwrap(), None)
        .await
        .unwrap();

    let retrieved: Option<TestData> = session.get("test").await.unwrap();
    assert_eq!(retrieved.unwrap(), test_data);

    let mut new_data = test_data.clone();
    new_data.f2 = "New Name".to_string();
    session
        .set("test", &new_data, Ttl::new(3600).unwrap(), None)
        .await
        .unwrap();

    let retrieved_new: Option<TestData> = session.get("test").await.unwrap();
    assert_eq!(retrieved_new.unwrap(), new_data);

    assert!(session.delete().await.unwrap());
    assert!(session.get::<TestData>("test").await.unwrap().is_none());
}

pub async fn run_session_uninitialized<S: SessionStore>(session: &Session<S>) {
    assert!(session.id().is_none());

    // Reads on an uninitialized session are None, not errors.
    assert!(session.get::<TestData>("x").await.unwrap().is_none());
    assert!(session.get_all().await.unwrap().is_none());

    // Mutations that need an id error out rather than minting one.
    assert!(session.remove("x").await.is_err());
    assert!(session.delete().await.is_err());
    assert!(
        session
            .expire_field("x", Ttl::new(60).unwrap())
            .await
            .is_err()
    );
}

pub async fn run_session_expire_field<S: SessionStore>(session: &Session<S>) {
    session
        .set("f", &create_test_data(), Ttl::new(60).unwrap(), None)
        .await
        .unwrap();
    assert!(
        session
            .expire_field("f", Ttl::new(120).unwrap())
            .await
            .unwrap()
    );
    assert!(
        !session
            .expire_field("ghost", Ttl::new(120).unwrap())
            .await
            .unwrap()
    );
}

pub async fn run_session_regenerate<S: SessionStore>(store: &S, session: &Session<S>) {
    session
        .set("k", &create_test_data(), Ttl::new(3600).unwrap(), None)
        .await
        .unwrap();
    let old = session.id().unwrap();

    let new = session
        .regenerate()
        .await
        .unwrap()
        .expect("regenerate should yield a new id");
    assert_ne!(old.to_string(), new.to_string());
    assert_eq!(session.id().unwrap().to_string(), new.to_string());

    assert_eq!(
        session.get::<TestData>("k").await.unwrap(),
        Some(create_test_data())
    );
    assert!(store.get::<TestData>(&old, "k").await.unwrap().is_none());
}

pub async fn run_session_prepare_regenerate<S: SessionStore>(store: &S, session: Session<S>) {
    let test_data = create_test_data();

    session
        .set("test1", &test_data, Ttl::new(3600).unwrap(), None)
        .await
        .unwrap();
    let original_id = session.id().unwrap();

    let prepared_id = session.prepare_regenerate();
    let mut new_data = test_data.clone();
    new_data.f2 = "New User".to_string();

    // This set both renames the session to the prepared id and writes test2.
    session
        .set("test2", &new_data, Ttl::new(3600).unwrap(), None)
        .await
        .unwrap();

    let current_id = session.id().unwrap();
    assert_eq!(current_id.to_string(), prepared_id.to_string());
    assert_ne!(current_id.to_string(), original_id.to_string());

    assert_eq!(
        session.get::<TestData>("test1").await.unwrap(),
        Some(test_data)
    );
    assert_eq!(
        session.get::<TestData>("test2").await.unwrap(),
        Some(new_data)
    );
    assert!(
        store
            .get::<TestData>(&original_id, "test1")
            .await
            .unwrap()
            .is_none()
    );
}

async fn ops_handler(session: Session<MokaStore>) -> Result<String, StatusCode> {
    run_session_operations(&session).await;
    run_session_expire_field(&session).await;
    // run_session_prepare_regenerate(&session).await;
    // run_session_regenerate(&session).await;
    Ok("ok".to_string())
}

async fn test_regen_handler(session: Session<MokaStore>) -> Result<String, StatusCode> {
    let test_data = create_test_data();

    session
        .set("test1", &test_data, Ttl::new(3600).unwrap(), None)
        .await
        .unwrap();
    let original_id = session.id().unwrap();

    let prepared_id = session.prepare_regenerate();
    let mut new_data = test_data.clone();
    new_data.f2 = "New User".to_string();

    // This set both renames the session to the prepared id and writes test2.
    session
        .set("test2", &new_data, Ttl::new(3600).unwrap(), None)
        .await
        .unwrap();

    let current_id = session.id().unwrap();
    assert_eq!(current_id.to_string(), prepared_id.to_string());
    assert_ne!(current_id.to_string(), original_id.to_string());

    let retrieved1: Option<TestData> = session.get("test1").await.unwrap();
    let retrieved2: Option<TestData> = session.get("test2").await.unwrap();
    assert_eq!(retrieved1.unwrap(), test_data);
    assert_eq!(retrieved2.unwrap(), new_data);

    Ok("ok".to_string())
}

pub fn create_axum_app() -> Router {
    let cookie_options = CookieOptions::build()
        .name("test_sess")
        .http_only(true)
        .same_site(cookie::SameSite::Lax)
        .secure(true)
        .max_age(COOKIE_MAX_AGE)
        .path("/");

    #[cfg(feature = "signed")]
    let cookie_options = cookie_options.signing_key(Key::generate());

    let store = Arc::new(MokaStoreBuilder::new().build());
    let session_layer = SessionLayer::new(store).with_cookie_options(cookie_options);

    Router::new()
        .route("/set", get(insert_handler))
        .route("/get", get(get_handler))
        .route("/delete", get(delete_handler))
        .route("/test_ops", get(ops_handler))
        .route("/test_regen", get(test_regen_handler))
        .layer(session_layer)
        .layer(CookieManagerLayer::new())
}

#[tokio::test]
async fn test_session_extraction_new_session() {
    let app = create_axum_app();

    let response = app
        .oneshot(Request::builder().uri("/set").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let cookie_header = response
        .headers()
        .get(SET_COOKIE)
        .expect("Set-Cookie header should be present");
    let cookie_str = cookie_header.to_str().unwrap();

    assert!(cookie_str.contains("test_sess="));
    assert!(cookie_str.contains("HttpOnly"));
    assert!(cookie_str.contains("SameSite=Lax"));
    // `Secure` is a valueless attribute; the cookie crate renders it as `Secure`.
    assert!(cookie_str.contains("Secure"));

    // Regression guard: the cookie Max-Age must come from CookieOptions (15),
    // NOT the field TTL (60). A field write does not drive cookie lifetime.
    assert!(
        cookie_str.contains(&format!("Max-Age={COOKIE_MAX_AGE}")),
        "cookie Max-Age must be the configured {COOKIE_MAX_AGE}, not the field TTL 60; got: {cookie_str}"
    );
}

#[tokio::test]
async fn test_session_extraction_with_existing_cookie() {
    let app = create_axum_app();

    // First request to get a session cookie
    let response = app
        .clone()
        .oneshot(Request::builder().uri("/set").body(Body::empty()).unwrap())
        .await
        .unwrap();

    let raw_set_cookie = response
        .headers()
        .get(SET_COOKIE)
        .expect("Set-Cookie header should be present")
        .to_str()
        .unwrap();

    let cookie = raw_set_cookie.split(';').next().unwrap().to_string();

    // Second request using the session cookie
    let response = app
        .oneshot(
            Request::builder()
                .uri("/get")
                .header(COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8(body.to_vec()).unwrap();

    assert_eq!(body_str, r#"{"f1":1,"f2":"Test"}"#);
}

#[tokio::test]
async fn test_logout_clears_cookie() {
    let app = create_axum_app();

    // 1. Establish a session; the client now "holds" the cookie.
    let response = app
        .clone()
        .oneshot(Request::builder().uri("/set").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let set_cookie = response
        .headers()
        .get(SET_COOKIE)
        .expect("Set-Cookie should be present on /set")
        .to_str()
        .unwrap();
    let cookie = set_cookie.split(';').next().unwrap().to_string();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/delete")
                .header(COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let cookie_str = response
        .headers()
        .get(SET_COOKIE)
        .expect("a clearing Set-Cookie should be present on logout")
        .to_str()
        .unwrap();

    assert!(cookie_str.contains("test_sess="));
    // A removal cookie carries an empty value and/or an expiry in the past.
    let clears = cookie_str.contains("Max-Age=0")
        || cookie_str.contains("test_sess=;")
        || cookie_str.to_ascii_lowercase().contains("expires");
    assert!(
        clears,
        "logout must emit a clearing cookie, got: {cookie_str}"
    );
}

#[tokio::test]
async fn test_missing_cookie_middleware() {
    // Create app without CookieManagerLayer
    let app = Router::new().route("/set", get(insert_handler)).layer(
        SessionLayer::new(Arc::new(MokaStoreBuilder::new().build()))
            .with_cookie_options(CookieOptions::build().name("test_sess")),
    );

    let response = app
        .oneshot(Request::builder().uri("/set").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn test_malformed_session_id() {
    let app = create_axum_app();

    // Try with malformed session ID
    let response = app
        .oneshot(
            Request::builder()
                .uri("/get")
                .header(COOKIE, "test_sess=invalid_session_id")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8(body.to_vec()).unwrap();

    assert_eq!(body_str, "null");
}

#[tokio::test]
async fn test_high_level_session_lifecycle() {
    let app = create_axum_app();

    let req1 = Request::builder()
        .uri("/test_ops")
        .body(Body::empty())
        .unwrap();
    let res1 = app.clone().oneshot(req1).await.unwrap();
    assert!(
        res1.status().is_success(),
        "Session ops handler panicked or failed"
    );

    let req2 = Request::builder()
        .uri("/test_regen")
        .body(Body::empty())
        .unwrap();
    let res2 = app.oneshot(req2).await.unwrap();
    assert!(
        res2.status().is_success(),
        "Session regen handler panicked or failed"
    );
}
