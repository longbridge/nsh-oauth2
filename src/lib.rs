//! OAuth 2 sign-in sessions for Native Shell apps (an nshm).
//!
//! The app opens a provider's sign-in page (an OAuth 2 authorization URL),
//! the user signs in, and the page redirects to a URL of the app's callback
//! scheme, which comes back to the app as the session's result
//! ([`open_auth_session`]). The app then exchanges the code it carries for
//! tokens with its own HTTP client; this crate does not talk to the
//! provider.
//!
//! How each platform shows the page:
//!
//! - iOS: `ASWebAuthenticationSession`, a sheet over the app that shares
//!   cookies with Safari unless the request asks for an ephemeral session.
//!   The system hands the callback URL back itself; the scheme needs no
//!   claim in `nsh.json`.
//! - Android: a Custom Tab (the default browser where it has none). The
//!   callback comes back as a link of a scheme the app claims (`nsh.json`
//!   `schemes`); the user coming back to the app without one cancels the
//!   session.
//! - macOS, Windows and Linux: the system browser, and the callback comes
//!   back as a link of a scheme the app claims, as on Android. Nothing tells
//!   the app the user gave up there, so such a session ends only with its
//!   callback, [`close_auth_session`] or a later session.
//! - The web: the page hears only its own address, so the session fails at
//!   once with [`UNAVAILABLE`].
//!
//! One session runs at a time: a new one ends the earlier as cancelled.
//! While a session waits on Android and the desktops, this crate listens
//! for the app's links ([`nsh_core::element::next_url_scheme_open`]) and
//! takes the callback; a Rust app that awaits that function itself should
//! not open a session at the same time.

mod request;

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "ios")]
mod ios;

use std::sync::{
    Mutex,
    atomic::{AtomicU64, Ordering},
};

use nsh_core::{platform::Os, prelude::*};
use tokio::sync::oneshot;

pub use request::{
    AuthSessionError, AuthSessionOutcome, AuthSessionRequest, MAX_SCHEME_LEN, UNAVAILABLE,
};

#[native_module("nsh-oauth2")]
fn native(registry: &mut Registry) {
    registry.module::<OAuth2>();
}

/// The session waiting for its end, if any: its id and what cancels it.
static CURRENT: Mutex<Option<(u64, oneshot::Sender<()>)>> = Mutex::new(None);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Starts session bookkeeping: cancels the earlier session, if any, and
/// returns this one's id and its cancel signal.
fn begin() -> (u64, oneshot::Receiver<()>) {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let (cancel, cancelled) = oneshot::channel();
    let earlier = CURRENT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .replace((id, cancel));
    if let Some((_, earlier)) = earlier {
        let _ = earlier.send(());
    }
    (id, cancelled)
}

/// Ends session `id`'s bookkeeping, unless a later one replaced it.
fn end(id: u64) {
    let mut current = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
    if current.as_ref().is_some_and(|(running, _)| *running == id) {
        *current = None;
    }
}

/// Opens a web sign-in session for `request` and resolves with how it
/// ended: the callback URL, or [`AuthSessionOutcome::Cancelled`]. Fails
/// with an [`AuthSessionError`] when the request is wrong
/// ([`AuthSessionRequest::check`]), when nothing here can show the page or
/// bring its callback back ([`UNAVAILABLE`]), or when the page could not be
/// shown. In a task.
pub async fn open_auth_session(
    request: AuthSessionRequest,
) -> Result<AuthSessionOutcome, AuthSessionError> {
    request.check()?;
    if os() == Os::Web {
        return Err(AuthSessionError::unavailable(
            "openAuthSession: a web page hears no links of the app's schemes, so no callback can come back",
        ));
    }
    let (id, cancelled) = begin();
    let result = platform::run(id, &request, cancelled).await;
    end(id);
    result
}

/// Closes the app's sign-in session while it waits: it ends as
/// [`AuthSessionOutcome::Cancelled`], and iOS closes its sheet. Does
/// nothing when no session waits. A browser tab the desktops or Android
/// opened stays until the user closes it.
pub fn close_auth_session() {
    let current = CURRENT.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some((_, cancel)) = current {
        let _ = cancel.send(());
    }
}

/// Waits for the callback link of `scheme` while the page shows in a
/// browser: links of other schemes are left to the log. Ends as cancelled
/// when `cancelled` fires.
#[cfg(not(target_os = "ios"))]
async fn wait_for_callback(
    scheme: &str,
    cancelled: &mut oneshot::Receiver<()>,
) -> Result<AuthSessionOutcome, AuthSessionError> {
    loop {
        tokio::select! {
            url = next_url_scheme_open() => {
                if request::is_callback(&url, scheme) {
                    return Ok(AuthSessionOutcome::Completed(url));
                }
                log::warn!("nsh-oauth2: a link of another scheme came while a sign-in session waited");
            }
            _ = &mut *cancelled => return Ok(AuthSessionOutcome::Cancelled),
        }
    }
}

/// The desktops: the system browser, the callback as a link.
#[cfg(not(any(target_os = "ios", target_os = "android")))]
mod platform {
    use super::*;

    pub(super) async fn run(
        _id: u64,
        request: &AuthSessionRequest,
        mut cancelled: oneshot::Receiver<()>,
    ) -> Result<AuthSessionOutcome, AuthSessionError> {
        if !open_url(request.url.clone()).await {
            return Err(AuthSessionError::failed(
                "openAuthSession: the browser did not open the sign-in page",
            ));
        }
        wait_for_callback(&request.scheme(), &mut cancelled).await
    }
}

#[cfg(target_os = "android")]
use self::android as platform;
#[cfg(target_os = "ios")]
use self::ios as platform;

/// Web sign-in sessions (OAuth 2 authorization pages).
#[module]
pub(crate) struct OAuth2;

#[module]
impl OAuth2 {
    /// Opens the sign-in page `url` and resolves with the callback URL the
    /// page redirected to (a URL of `callbackScheme`), or `null` when the
    /// user closed the page or came back without signing in, the app closed
    /// the session, or a later session replaced it. iOS shows
    /// `ASWebAuthenticationSession`, Android a Custom Tab, the desktops the
    /// system browser; on Android and the desktops the callback scheme must
    /// be one the app claims (`nsh.json` `schemes`). Rejects with
    /// `ERR_FEATURE_UNAVAILABLE_ON_PLATFORM` on the web, and with an `Error`
    /// for a wrong URL or scheme.
    pub(crate) async fn open_auth_session(
        options: OpenAuthSessionOptions,
    ) -> Result<Option<String>, JsError> {
        let request = AuthSessionRequest::new(options.url, options.callback_scheme)
            .ephemeral(options.ephemeral.unwrap_or(false));
        match crate::open_auth_session(request).await {
            Ok(AuthSessionOutcome::Completed(url)) => Ok(Some(url)),
            Ok(AuthSessionOutcome::Cancelled) => Ok(None),
            Err(error) => Err(error.to_js()),
        }
    }

    /// Closes the sign-in session while it waits: its `openAuthSession`
    /// resolves with `null`. Does nothing when none waits.
    pub(crate) async fn close_auth_session() {
        crate::close_auth_session();
    }
}

/// What `openAuthSession` opens.
#[derive(Debug, Clone, Default, PartialEq, JsObject)]
#[prop(exact)]
pub(crate) struct OpenAuthSessionOptions {
    /// The sign-in page: an `https:` (or `http:`) URL, such as an OAuth 2
    /// authorization URL.
    pub(crate) url: String,
    /// The scheme of the callback URL, without `:` (`myapp` for
    /// `myapp://signed-in?code=...`).
    pub(crate) callback_scheme: String,
    /// Share no cookies or other data with the user's browsing (off when not
    /// given).
    pub(crate) ephemeral: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_session_cancels_the_earlier() {
        let (first, mut first_cancelled) = begin();
        let (second, mut second_cancelled) = begin();
        assert!(first_cancelled.try_recv().is_ok());
        assert!(second_cancelled.try_recv().is_err());
        // The earlier one ending leaves the later one waiting.
        end(first);
        close_auth_session();
        assert!(second_cancelled.try_recv().is_ok());
        end(second);
        assert!(CURRENT.lock().unwrap().is_none());
    }
}
