//! What a sign-in session asks for and how it ends: [`AuthSessionRequest`]
//! and its check, [`AuthSessionOutcome`], [`AuthSessionError`].

use std::fmt;

use nsh_core::{element::codes, prelude::*};

/// The code of a session that cannot run here: nothing shows the page or
/// brings its callback back (the web, a terminal).
pub const UNAVAILABLE: &str = codes::UNAVAILABLE;

/// The most bytes of a callback scheme.
pub const MAX_SCHEME_LEN: usize = 64;

/// Schemes a callback never has: web addresses (a page redirects to them on
/// its own, so they do not end a session) and schemes that run code or name
/// local files.
const REFUSED_SCHEMES: &[&str] = &[
    "http",
    "https",
    "file",
    "javascript",
    "vbscript",
    "data",
    "blob",
    "about",
    "asset",
];

/// A sign-in session the app asks for: the page to open and the scheme of
/// the URL it redirects to when the user is done.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct AuthSessionRequest {
    /// The sign-in page: an `https:` (or `http:`) URL, such as an OAuth 2
    /// authorization URL.
    pub url: String,
    /// The scheme of the callback URL, without `:` (`myapp` for
    /// `myapp://signed-in?code=...`).
    pub callback_scheme: String,
    /// Asks the browser not to share cookies or other data with the user's
    /// browsing (iOS's `prefersEphemeralWebBrowserSession`; ephemeral
    /// browsing in Android browsers whose Custom Tabs offer it). Off by
    /// default.
    pub ephemeral: bool,
}

impl AuthSessionRequest {
    /// A session opening `url` and ending with a URL of `callback_scheme`.
    pub fn new(url: impl Into<String>, callback_scheme: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            callback_scheme: callback_scheme.into(),
            ephemeral: false,
        }
    }

    /// Asks for a session that shares nothing with the user's browsing.
    pub fn ephemeral(mut self, ephemeral: bool) -> Self {
        self.ephemeral = ephemeral;
        self
    }

    /// Checks the request: an `https:` or `http:` URL; a callback scheme of
    /// at most [`MAX_SCHEME_LEN`] bytes, a letter then letters, digits, `+`,
    /// `-` or `.`, and not a web or local-file scheme. The error says what is
    /// wrong.
    pub fn check(&self) -> Result<(), AuthSessionError> {
        let failed = |message: String| Err(AuthSessionError::failed(message));
        let parsed = match url::Url::parse(&self.url) {
            Ok(parsed) if !self.url.chars().any(char::is_whitespace) => parsed,
            Ok(_) => return failed("openAuthSession: the URL has spaces".into()),
            Err(e) => return failed(format!("openAuthSession: not a URL: {e}")),
        };
        if !matches!(parsed.scheme(), "https" | "http") {
            return failed(format!(
                "openAuthSession: the sign-in page is a web address, not a `{}:` URL",
                parsed.scheme()
            ));
        }
        let scheme = &self.callback_scheme;
        if scheme.len() > MAX_SCHEME_LEN {
            return failed(format!(
                "openAuthSession: a callback scheme has at most {MAX_SCHEME_LEN} bytes"
            ));
        }
        let mut chars = scheme.chars();
        let valid = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
            && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
        if !valid {
            return failed(format!(
                "openAuthSession: `{scheme}` is not a URL scheme (give it without `:`)"
            ));
        }
        if REFUSED_SCHEMES.contains(&scheme.to_ascii_lowercase().as_str()) {
            return failed(format!(
                "openAuthSession: `{scheme}:` is not a callback scheme (use one of the app's own)"
            ));
        }
        Ok(())
    }

    /// The callback scheme as URLs carry it (lower case).
    pub(crate) fn scheme(&self) -> String {
        self.callback_scheme.to_ascii_lowercase()
    }
}

/// How a sign-in session ended.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AuthSessionOutcome {
    /// The page redirected to this callback URL (of the request's scheme).
    Completed(String),
    /// The user closed the page or came back to the app without signing
    /// in, the app closed the session ([`crate::close_auth_session`]), or a
    /// later session replaced this one.
    Cancelled,
}

/// Why a sign-in session could not run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct AuthSessionError {
    /// A stable code the app can branch on ([`UNAVAILABLE`]); `None` for
    /// other failures.
    pub code: Option<String>,
    /// What went wrong, in words.
    pub message: String,
}

impl AuthSessionError {
    /// A failure with `message` and no code.
    pub fn failed(message: impl Into<String>) -> Self {
        Self {
            code: None,
            message: message.into(),
        }
    }

    /// Nothing here shows the page or brings its callback back
    /// ([`UNAVAILABLE`]).
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self {
            code: Some(UNAVAILABLE.into()),
            message: message.into(),
        }
    }

    /// How TSX gets it: an `Error` with its `code`.
    pub(crate) fn to_js(&self) -> JsError {
        match &self.code {
            Some(code) => JsError::Error(self.message.clone()).with_code(code),
            None => JsError::Error(self.message.clone()),
        }
    }
}

impl fmt::Display for AuthSessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AuthSessionError {}

/// Whether `url` is a callback of a session waiting for `scheme` (lower
/// case).
pub(crate) fn is_callback(url: &str, scheme: &str) -> bool {
    url::Url::parse(url).is_ok_and(|parsed| parsed.scheme() == scheme)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_opens_a_web_page_and_ends_with_an_app_scheme() {
        let ok = AuthSessionRequest::new("https://example.com/authorize?x=1", "com.example.app");
        assert!(ok.check().is_ok());
        assert!(
            AuthSessionRequest::new("http://127.0.0.1:8080/authorize", "my-app+1")
                .check()
                .is_ok()
        );
        for (url, scheme) in [
            ("myapp://authorize", "myapp"),
            ("javascript:alert(1)", "myapp"),
            ("https://example.com/a b", "myapp"),
            ("https://example.com", ""),
            ("https://example.com", "myapp:"),
            ("https://example.com", "1app"),
            ("https://example.com", "https"),
            ("https://example.com", "HTTP"),
            ("https://example.com", "file"),
        ] {
            assert!(
                AuthSessionRequest::new(url, scheme).check().is_err(),
                "{url} {scheme}"
            );
        }
        let long = "a".repeat(MAX_SCHEME_LEN + 1);
        assert!(
            AuthSessionRequest::new("https://example.com", long)
                .check()
                .is_err()
        );
    }

    #[test]
    fn a_callback_is_a_url_of_the_scheme() {
        let request = AuthSessionRequest::new("https://example.com", "MyApp");
        assert_eq!(request.scheme(), "myapp");
        assert!(is_callback("MYAPP://done?code=1", &request.scheme()));
        assert!(!is_callback("other://done", &request.scheme()));
        assert!(!is_callback("not a url", &request.scheme()));
    }

    #[test]
    fn errors_keep_their_codes() {
        let error = AuthSessionError::unavailable("no browser");
        assert_eq!(error.code.as_deref(), Some(UNAVAILABLE));
        assert_eq!(error.to_string(), "no browser");
        assert_eq!(AuthSessionError::failed("x").code, None);
    }
}
