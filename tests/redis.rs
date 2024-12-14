mod common;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        extract::Request,
        http::{self, StatusCode},
        routing::get,
        Router,
    };
    use common::*;
    use fred::{clients::Client, interfaces::ClientLike};
    use http::header::{COOKIE, SET_COOKIE};
    use ruts::{Session, SessionLayer};
    use ruts::store::redis::RedisStore;
    use std::sync::Arc;
    use tower::ServiceExt;
    use tower_cookies::CookieManagerLayer;

    async fn setup_redis() -> Arc<RedisStore<Client>> {
        let client = Client::default();
        client.connect();
        client.wait_for_connect().await.unwrap();
        Arc::new(RedisStore::new(Arc::new(client)))
    }

    async fn insert_handler(session: Session<RedisStore<Client>>) -> Result<String, StatusCode> {
        let test_data = create_test_session();
        session
            .insert("user", &test_data)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Ok("Success".to_string())
    }

    async fn get_handler(session: Session<RedisStore<Client>>) -> Result<String, StatusCode> {
        let data: Option<TestSession> = session
            .get("user")
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        Ok(data.map(|d| d.user.name).unwrap_or_else(|| "Not found".to_string()))
    }

    async fn regenerate_handler(session: Session<RedisStore<Client>>) -> Result<String, StatusCode> {
        session
            .regenerate()
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Ok("Regenerated".to_string())
    }

    async fn delete_handler(session: Session<RedisStore<Client>>) -> Result<String, StatusCode> {
        session
            .delete()
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Ok("Deleted".to_string())
    }

    fn create_test_app(store: Arc<RedisStore<Client>>) -> Router {
        let session_layer = SessionLayer::new(store)
            .with_cookie_options(build_cookie_options());

        Router::new()
            .route("/set", get(insert_handler))
            .route("/get", get(get_handler))
            .route("/regenerate", get(regenerate_handler))
            .route("/delete", get(delete_handler))
            .layer(session_layer)
            .layer(CookieManagerLayer::new())
    }

    #[tokio::test]
    async fn test_redis_session_lifecycle() {
        let store = setup_redis().await;
        let app = create_test_app(store);

        // Create new session
        let response = app
            .clone()
            .oneshot(Request::builder().uri("/set").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let cookie = response
            .headers()
            .get(SET_COOKIE)
            .expect("Set-Cookie header should be present")
            .to_str()
            .unwrap()
            .to_string();

        // Verify data was stored
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/get")
                    .header(COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        assert_eq!(body_str, "Test User");

        // Regenerate session
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/regenerate")
                    .header(COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let new_cookie = response
            .headers()
            .get(SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        assert_ne!(cookie, new_cookie);

        // Verify data persists after regeneration
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/get")
                    .header(COOKIE, &new_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        assert_eq!(body_str, "Test User");

        // Delete session
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/delete")
                    .header(COOKIE, &new_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        // Verify data is gone
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
        assert_eq!(body_str, "Not found");
    }

    #[tokio::test]
    async fn test_concurrent_session_access() {
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

        // Simulate concurrent access
        let mut handles = Vec::new();
        for _ in 0..5 {
            let app = app.clone();
            let cookie = cookie.clone();

            handles.push(tokio::spawn(async move {
                app.oneshot(
                    Request::builder()
                        .uri("/get")
                        .header(COOKIE, &cookie)
                        .body(Body::empty())
                        .unwrap(),
                )
                    .await
                    .unwrap()
            }));
        }

        // Wait for all requests to complete
        for handle in handles {
            let response = handle.await.unwrap();
            let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let body_str = String::from_utf8(body.to_vec()).unwrap();
            assert_eq!(body_str, "Test User");
        }
    }
}
