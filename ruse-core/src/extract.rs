use std::sync::Arc;

use async_trait::async_trait;
use axum_core::extract::FromRequestParts;
use cookie::Cookie;
use http::{request::Parts, StatusCode};
use tower_cookies::Cookies;

use crate::{Id, InnerSessionUtil, Session};

/// Axum Extractor for [`Session`].
#[async_trait]
impl<S, T> FromRequestParts<S> for Session<T>
where
    S: Sync + Send,
    T: Clone + Default + Send + Sync,
{
    type Rejection = (http::StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let inner_session_utils = parts
            .extensions
            .get::<Arc<InnerSessionUtil>>()
            .cloned()
            .ok_or((
                StatusCode::INTERNAL_SERVER_ERROR,
                "SessionLayer not found in the request extensions",
            ))?;

        // Cookies are only used if the SessionLayer has a cookie_options set.
        // Hence there is no overhead incurred if the SessionLayer does not have a cookie_options set.
        let session = if let Some(cookie_options) = inner_session_utils.cookie_options {
            let cookies = parts.extensions.get::<Cookies>().cloned();
            if let Some(cookies) = cookies {
                if let Some(cookie) = cookies.get(cookie_options.name).map(Cookie::into_owned) {
                    let session_id = cookie.clone().value().parse::<Id>().ok();

                    if let Some(id) = session_id {
                        Session::new(inner_session_utils.clone(), Some(id))
                    } else {
                        Session::new(inner_session_utils.clone(), None)
                    }
                } else {
                    Session::new(inner_session_utils.clone(), None)
                }
            } else {
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Cookies not found in the request extensions",
                ));
            }
        } else {
            Session::new(inner_session_utils.clone(), None)
        };

        Ok(session)
    }
}
