use serde::{Deserialize, Serialize};
use ruts::CookieOptions;

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct TestUser {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct TestSession {
    pub user: TestUser,
    pub preferences: TestPreferences,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct TestPreferences {
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
        .max_age(3600)
        .path("/")
}
