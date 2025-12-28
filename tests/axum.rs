use ruts::CookieOptions;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub(crate) struct TestUser {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct TestSession {
    pub user: TestUser,
    pub preferences: TestPreferences,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub(crate) struct TestPreferences {
    pub theme: String,
    pub language: String,
}

pub fn create_test_session() -> TestSession {
    TestSession {
        user: TestUser {
            id: 1,
            name: "Test User".to_string(),
        },
        preferences: TestPreferences {
            theme: "dark".to_string(),
            language: "en".to_string(),
        },
    }
}

pub fn build_cookie_options() -> CookieOptions {
    CookieOptions::build()
        .name("test_sess")
        .http_only(true)
        .same_site(cookie::SameSite::Lax)
        .secure(true)
        .max_age(15)
        .path("/")
}


#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::Body,
        extract::Request,
        http::{self, StatusCode},
        routing::get,
    };
    use http::header::{COOKIE, SET_COOKIE};
    use ruts::store::memory::MemoryStore;
    use ruts::{CookieOptions, Session, SessionLayer};
    use std::sync::Arc;
    use tower::ServiceExt;
    use tower_cookies::CookieManagerLayer;

    // Test handler that requires Session
    async fn insert_handler(session: Session<MemoryStore>) -> Result<String, StatusCode> {
        let user = TestUser {
            id: 1,
            name: "Test".to_string(),
        };
        session
            .set("user", &user, Some(20), None)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Ok("Success".to_string())
    }

    async fn get_handler(session: Session<MemoryStore>) -> Result<String, StatusCode> {
        let user: Option<TestUser> = session
            .get("user")
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Ok(user
            .map(|u| u.name)
            .unwrap_or_else(|| "Not found".to_string()))
    }

    fn create_test_app() -> Router {
        let cookie_options = build_cookie_options();
        let session_layer =
            SessionLayer::new(Arc::new(MemoryStore::new())).with_cookie_options(cookie_options);

        Router::new()
            .route("/set", get(insert_handler))
            .route("/get", get(get_handler))
            .layer(session_layer)
            .layer(CookieManagerLayer::new())
    }

    #[tokio::test]
    async fn test_session_extraction_new_session() {
        let app = create_test_app();

        let response = app
            .oneshot(Request::builder().uri("/set").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        // Verify Set-Cookie header exists and has correct attributes
        let cookie_header = response
            .headers()
            .get(SET_COOKIE)
            .expect("Set-Cookie header should be present");

        let cookie_str = cookie_header.to_str().unwrap();
        assert!(cookie_str.contains("test_sess="));
        assert!(cookie_str.contains("HttpOnly"));
        assert!(cookie_str.contains("Secure"));
        assert!(cookie_str.contains("SameSite=Lax"));
    }

    #[tokio::test]
    async fn test_session_extraction_with_existing_cookie() {
        let app = create_test_app();

        // First request to get a session cookie
        let response = app
            .clone()
            .oneshot(Request::builder().uri("/set").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let cookie = response
            .headers()
            .get(SET_COOKIE)
            .expect("Set-Cookie header should be present")
            .to_str()
            .unwrap()
            .to_string();

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
        assert_eq!(body_str, "Test");
    }
    #[tokio::test]
    async fn test_missing_cookie_middleware() {
        // Create app without CookieManagerLayer
        let app = Router::new().route("/set", get(insert_handler)).layer(
            SessionLayer::new(Arc::new(MemoryStore::new()))
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
        let app = create_test_app();

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

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        assert_eq!(body_str, "Not found");
    }
}
