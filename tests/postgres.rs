#![cfg(feature = "postgres-store")]

mod common;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        extract::Request,
        http::{self, StatusCode},
        routing::get,
        Json, Router,
    };
    use common::*;
    use cookie::Cookie;
    use http::header::{COOKIE, SET_COOKIE};
    use ruts::store::postgres::{PostgresStore, PostgresStoreBuilder};
    use ruts::{Session, SessionLayer};
    use sqlx::PgPool;
    use std::sync::Arc;
    use time::Duration;
    use tower::ServiceExt;
    use tower_cookies::CookieManagerLayer;

    async fn setup_postgres() -> Arc<PostgresStore> {
        let database_url =
            std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for tests");
        let pool = PgPool::connect(&database_url).await.unwrap();

        // Clean up table before each test run for test isolation
        sqlx::query("drop table if exists sessions")
            .execute(&pool)
            .await
            .unwrap();

        let store = PostgresStoreBuilder::new(pool.clone())
            .build()
            .await
            .unwrap();
        Arc::new(store)
    }

    /// Sets up a connection to a test Postgres database with custom schema and table.
    async fn setup_postgres_custom(schema: &str, table: &str) -> Arc<PostgresStore> {
        let database_url =
            std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for tests");
        let pool = PgPool::connect(&database_url).await.unwrap();

        sqlx::query(&format!("drop schema if exists \"{}\" cascade", schema))
            .execute(&pool)
            .await
            .unwrap();

        let store = PostgresStoreBuilder::new(pool.clone())
            .schema_name(schema)
            .table_name(table)
            .build()
            .await
            .unwrap();
        Arc::new(store)
    }

    async fn insert_handler(session: Session<PostgresStore>) -> Result<String, StatusCode> {
        let test_data = create_test_session();
        session
            .insert("user", &test_data.user, Some(60))
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        session
            .insert("preferences", &test_data.preferences, Some(60))
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Ok("Success".to_string())
    }

    async fn get_handler(session: Session<PostgresStore>) -> Result<String, StatusCode> {
        let data: Option<TestUser> = session
            .get("user")
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Ok(data
            .map(|d| d.name)
            .unwrap_or_else(|| "Not found".to_string()))
    }

    async fn get_all_handler(
        session: Session<PostgresStore>,
    ) -> Result<Json<TestSession>, StatusCode> {
        let data = session
            .get_all()
            .await
            .map_err(|err| {
                println!("{:?}", err);
                StatusCode::INTERNAL_SERVER_ERROR
            })?
            .unwrap();

        Ok(Json(TestSession {
            user: data.get("user").unwrap().unwrap(),
            preferences: data.get("preferences").unwrap().unwrap(),
        }))
    }

    async fn regenerate_handler(session: Session<PostgresStore>) -> Result<String, StatusCode> {
        session
            .regenerate()
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Ok("Regenerated".to_string())
    }

    async fn prepare_regenerate_handler(
        session: Session<PostgresStore>,
    ) -> Result<String, StatusCode> {
        session.prepare_regenerate();
        let mut updated_data = create_test_session();
        updated_data.user.name = "Updated User".to_string();
        session
            .update("user", &updated_data.user, None)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Ok("Success".to_string())
    }

    async fn set_expiration_handler(session: Session<PostgresStore>) -> Result<String, StatusCode> {
        session.set_expiration(30);
        let updated_data = create_test_session();
        session
            .update("user", &updated_data.user, Some(30))
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Ok("Success".to_string())
    }

    async fn delete_handler(session: Session<PostgresStore>) -> Result<String, StatusCode> {
        session
            .delete()
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Ok("Deleted".to_string())
    }

    fn create_test_app(store: Arc<PostgresStore>) -> Router {
        let session_layer = SessionLayer::new(store).with_cookie_options(build_cookie_options());
        Router::new()
            .route("/set", get(insert_handler))
            .route("/get", get(get_handler))
            .route("/get_all", get(get_all_handler))
            .route("/regenerate", get(regenerate_handler))
            .route("/prepare_regenerate", get(prepare_regenerate_handler))
            .route("/set_expiration", get(set_expiration_handler))
            .route("/delete", get(delete_handler))
            .layer(session_layer)
            .layer(CookieManagerLayer::new())
    }

    #[tokio::test]
    async fn test_postgres_session_lifecycle() {
        let store = setup_postgres().await;
        let app = create_test_app(store);

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

        // Verify data was stored.
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
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        assert_eq!(body_str, "Test User");

        // Verify the entire session object.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/get_all")
                    .header(COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let session_data: TestSession = serde_json::from_slice(&body).unwrap();
        assert_eq!(session_data, create_test_session());

        // Regenerate the session ID.
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

        // Verify data persists after regeneration.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/get_all")
                    .header(COOKIE, &new_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let session_data: TestSession = serde_json::from_slice(&body).unwrap();
        assert_eq!(session_data, create_test_session());

        // Delete the session.
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

        // Verify the session data is gone.
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
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        assert_eq!(body_str, "Not found");
    }

    #[tokio::test]
    async fn test_postgres_custom_schema_and_table() {
        let store = setup_postgres_custom("my_schema", "my_table").await;
        let app = create_test_app(store);

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

        // Verify data was stored.
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
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        assert_eq!(body_str, "Test User");
    }

    #[tokio::test]
    async fn test_prepare_regenerate_flow() {
        let store = setup_postgres().await;
        let app = create_test_app(store);

        let response = app
            .clone()
            .oneshot(Request::builder().uri("/set").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let original_cookie = response
            .headers()
            .get(SET_COOKIE)
            .expect("Set-Cookie header should be present")
            .to_str()
            .unwrap()
            .to_string();

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/prepare_regenerate")
                    .header(COOKIE, &original_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let new_cookie = response
            .headers()
            .get(SET_COOKIE)
            .expect("Set-Cookie header should be present")
            .to_str()
            .unwrap()
            .to_string();

        assert_ne!(
            original_cookie, new_cookie,
            "Session ID should have changed after prepare_regenerate"
        );
    }

    #[tokio::test]
    async fn test_set_expiration() {
        let store = setup_postgres().await;
        let app = create_test_app(store);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/set_expiration")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let new_cookie = response
            .headers()
            .get(SET_COOKIE)
            .expect("Set-Cookie header should be present")
            .to_str()
            .unwrap()
            .to_string();

        let parsed_cookie = Cookie::parse_encoded(&new_cookie).expect("Should be a valid cookie");
        assert_eq!(parsed_cookie.max_age(), Some(Duration::seconds(30)));
    }
}
