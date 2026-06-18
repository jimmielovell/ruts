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
use ruts::{CookieOptions, Session, SessionLayer};
use std::sync::Arc;
use tower::ServiceExt;
use tower_cookies::CookieManagerLayer;

async fn insert_handler(session: Session<MokaStore>) -> Result<String, StatusCode> {
    let data = create_test_data();

    session
        .set("data", &data, Some(60), None)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok("Success".to_string())
}

async fn get_handler(
    session: Session<MokaStore>,
) -> Result<Json<Option<TestData>>, StatusCode> {
    let data: Option<TestData> = session
        .get("data")
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(data))
}

async fn test_ops_handler(session: Session<MokaStore>) -> Result<String, StatusCode> {
    let test_data = create_test_data();

    let inserted = session.set("test", &test_data, None, None).await.unwrap();
    assert!(inserted);

    let retrieved: Option<TestData> = session.get("test").await.unwrap();
    assert_eq!(retrieved.unwrap(), test_data);

    let mut new_data = test_data.clone();
    new_data.f2 = "New Name".to_string();

    let inserted_again = session.set("test", &new_data, None, None).await.unwrap();
    assert!(inserted_again, "Insert should succeed (overwrite)");

    let retrieved_new: Option<TestData> = session.get("test").await.unwrap();
    assert_eq!(retrieved_new.unwrap(), new_data);

    let deleted = session.delete().await.unwrap();
    assert!(deleted);

    let retrieved: Option<TestData> = session.get("test").await.unwrap();
    assert!(retrieved.is_none());

    Ok("ok".to_string())
}

async fn test_regen_handler(
    session: Session<MokaStore>,
) -> Result<String, StatusCode> {
    let test_data = create_test_data();

    session.set("test1", &test_data, None, None).await.unwrap();
    let original_id = session.id().unwrap();

    let prepared_id = session.prepare_regenerate();
    let mut new_data = test_data.clone();
    new_data.f2 = "New User".to_string();

    let inserted = session.set("test2", &new_data, None, None).await.unwrap();
    assert!(inserted);

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
        .max_age(15)
        .path("/");

    #[cfg(feature = "signed")]
    let cookie_options = cookie_options.signing_key(Key::generate());

    let store = Arc::new(MokaStoreBuilder::new().build());
    let session_layer = SessionLayer::new(store).with_cookie_options(cookie_options);

    Router::new()
        .route("/set", get(insert_handler))
        .route("/get", get(get_handler))
        .route("/test_ops", get(test_ops_handler))
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
    assert!(cookie_str.contains("Secure=true"));
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