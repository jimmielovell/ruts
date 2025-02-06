use cookie::SameSite;

/// Configuration options for session cookies.
///
/// # Example
///
/// ```rust
/// use ruts::CookieOptions;
///
/// let cookie_options = CookieOptions::build()
///         .name("test_sess")
///         .http_only(true)
///         .same_site(cookie::SameSite::Lax)
///         .secure(true)
///         .max_age(1 * 60)
///         .path("/");
/// ```
#[derive(Clone, Copy, Debug)]
pub struct CookieOptions {
    pub http_only: bool,
    pub name: &'static str,
    pub domain: Option<&'static str>,
    pub path: Option<&'static str>,
    pub same_site: SameSite,
    pub secure: bool,
    pub max_age: i64,
}

impl Default for CookieOptions {
    fn default() -> Self {
        Self {
            http_only: true,
            name: "id",
            domain: None,
            path: None,
            same_site: SameSite::Lax,
            secure: true,
            max_age: 10 * 60,
        }
    }
}

impl CookieOptions {
    /// Creates a new `CookieOptions` with default values.
    pub fn build() -> Self {
        Self::default()
    }

    /// Sets the name of the cookie.
    pub fn name(mut self, name: &'static str) -> Self {
        self.name = name;
        self
    }

    pub fn http_only(mut self, http_only: bool) -> Self {
        self.http_only = http_only;
        self
    }

    pub fn same_site(mut self, same_site: SameSite) -> Self {
        self.same_site = same_site;
        self
    }

    pub fn secure(mut self, secure: bool) -> Self {
        self.secure = secure;
        self
    }

    pub fn domain(mut self, domain: &'static str) -> Self {
        self.domain = Some(domain);
        self
    }

    pub fn path(mut self, path: &'static str) -> Self {
        self.path = Some(path);
        self
    }

    pub fn max_age(mut self, seconds: i64) -> Self {
        self.max_age = seconds;
        self
    }
}
