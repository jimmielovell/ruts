#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        extract::Request,
        http::{self, StatusCode},
        routing::get,
        Router,
    };
    use fred::clients::Client;
    use http::header::{COOKIE, SET_COOKIE};
    use serde::{Deserialize, Serialize};
    use std::sync::Arc;
    use fred::interfaces::ClientLike;
    use tower::ServiceExt;
    use tower_cookies::CookieManagerLayer;
    use ruts::{CookieOptions, Session, SessionLayer};
    use ruts::store::redis::RedisStore;

    #[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
    struct TestUser {
        id: i64,
        name: String,
    }

    async fn setup_redis() -> Arc<RedisStore<Client>> {
        let client = Client::default();
        client.connect();
        client.wait_for_connect().await.unwrap();
        Arc::new(RedisStore::new(Arc::new(client)))
    }

    // Test handler that requires Session
    async fn test_handler(session: Session<RedisStore<Client>>) -> Result<String, StatusCode> {
        let user = TestUser {
            id: 1,
            name: "Test".to_string(),
        };
        session.insert("user", &user).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Ok("Success".to_string())
    }

    // Test handler that reads session data
    async fn get_session_data(session: Session<RedisStore<Client>>) -> Result<String, StatusCode> {
        let user: Option<TestUser> = session.get("user").await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Ok(user.map(|u| u.name).unwrap_or_else(|| "Not found".to_string()))
    }

    // Test handler that regenerates session
    async fn regenerate_session(session: Session<RedisStore<Client>>) -> Result<String, StatusCode> {
        session.regenerate().await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Ok("Regenerated".to_string())
    }

    // Test handler that deletes session
    async fn delete_session(session: Session<RedisStore<Client>>) -> Result<String, StatusCode> {
        session.delete().await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Ok("Deleted".to_string())
    }

    fn create_test_app(store: Arc<RedisStore<Client>>) -> Router {
        let cookie_options = CookieOptions::build()
            .name("test_sess")
            .http_only(true)
            .same_site(cookie::SameSite::Lax)
            .secure(true)
            .max_age(3600)
            .path("/");

        let session_layer = SessionLayer::new(store)
            .with_cookie_options(cookie_options);

        Router::new()
            .route("/set", get(test_handler))
            .route("/get", get(get_session_data))
            .route("/regenerate", get(regenerate_session))
            .route("/delete", get(delete_session))
            .layer(CookieManagerLayer::new())
            .layer(session_layer)
    }

    #[tokio::test]
    async fn test_session_extraction_new_session() {
        let store = setup_redis().await;
        let app = create_test_app(store);

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
        let store = setup_redis().await;
        let app = create_test_app(store);

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

        // Convert response body to string
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        assert_eq!(body_str, "Test");
    }

    #[tokio::test]
    async fn test_session_regeneration() {
        let store = setup_redis().await;
        let app = create_test_app(store);

        // First request to get a session cookie
        let response = app
            .clone()
            .oneshot(Request::builder().uri("/set").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let original_cookie = response
            .headers()
            .get(SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        // Regenerate session
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/regenerate")
                    .header(COOKIE, &original_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let new_cookie = response
            .headers()
            .get(SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        // Verify that we got a new session ID
        assert_ne!(original_cookie, new_cookie);

        // Verify that session data persisted
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/get")
                    .header(COOKIE, new_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        assert_eq!(body_str, "Test");
    }

    #[tokio::test]
    async fn test_session_deletion() {
        let store = setup_redis().await;
        let app = create_test_app(store);

        // Create session
        let response = app
            .clone()
            .oneshot(Request::builder().uri("/set").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let cookie = response
            .headers()
            .get(SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        // Delete session
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/delete")
                    .header(COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        // Verify session data is gone
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

        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        assert_eq!(body_str, "Not found");
    }

    #[tokio::test]
    async fn test_malformed_session_id() {
        let store = setup_redis().await;
        let app = create_test_app(store);

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

        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        assert_eq!(body_str, "Not found");
    }

    #[tokio::test]
    async fn test_missing_cookie_middleware() {
        let store = setup_redis().await;

        // Create app without CookieManagerLayer
        let app = Router::new()
            .route("/set", get(test_handler))
            .layer(SessionLayer::new(store)
                .with_cookie_options(CookieOptions::build()
                    .name("test_sess")));

        let response = app
            .oneshot(Request::builder().uri("/set").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
