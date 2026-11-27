//! iOS: `ASWebAuthenticationSession`, presented over the window of the
//! surface that asked (`nsh_platform_ios::presenting_controller`). Its
//! completion is the session's answer: the callback URL, a cancellation (the
//! user closed the page or declined the alert asking to sign in), or why it
//! failed. The app closing the session, or a later one, cancels the sheet
//! still showing.

use std::{cell::RefCell, rc::Rc};

use block2::RcBlock;
use nsh_core::prelude::*;
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained, runtime::ProtocolObject,
};
use objc2_authentication_services::{
    ASWebAuthenticationPresentationContextProviding, ASWebAuthenticationSession,
    ASWebAuthenticationSessionErrorCode, ASWebAuthenticationSessionErrorDomain,
};
use objc2_foundation::{NSError, NSObject, NSObjectProtocol, NSString, NSURL};
use objc2_ui_kit::UIWindow;
use tokio::sync::oneshot;

use crate::{AuthSessionError, AuthSessionOutcome, AuthSessionRequest};

type Answer = oneshot::Sender<Result<AuthSessionOutcome, AuthSessionError>>;

/// What a session's provider holds: the window it shows over.
struct ProviderIvars {
    window: Retained<UIWindow>,
}

define_class!(
    /// Tells the session which window to show over.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ProviderIvars]
    struct Provider;

    unsafe impl NSObjectProtocol for Provider {}

    // The protocol's one method is bound only for macOS (its anchor type is
    // `UIWindow` here), so the trait has no item for it; it is still defined
    // in this block, which registers it for the protocol.
    unsafe impl ASWebAuthenticationPresentationContextProviding for Provider {
        #[unsafe(method_id(presentationAnchorForWebAuthenticationSession:))]
        fn anchor(&self, _session: &ASWebAuthenticationSession) -> Retained<UIWindow> {
            self.ivars().window.clone()
        }
    }
);

/// The session showing, if any.
struct Running {
    id: u64,
    session: Retained<ASWebAuthenticationSession>,
    /// The session holds its provider weakly.
    _provider: Retained<Provider>,
}

thread_local! {
    /// The session the app shows (main thread only).
    static CURRENT: RefCell<Option<Running>> = const { RefCell::new(None) };
}

pub(crate) async fn run(
    id: u64,
    request: &AuthSessionRequest,
    cancelled: oneshot::Receiver<()>,
) -> Result<AuthSessionOutcome, AuthSessionError> {
    let (answer, answered) = oneshot::channel();
    let (url, scheme, ephemeral) = (request.url.clone(), request.scheme(), request.ephemeral);
    if let Err(error) = run_on_main_thread(move || start(id, url, scheme, ephemeral, answer)).await
    {
        return Err(AuthSessionError::failed(format!(
            "openAuthSession: the sign-in page cannot be shown: {error}"
        )));
    }
    tokio::select! {
        answer = answered => answer.unwrap_or(Ok(AuthSessionOutcome::Cancelled)),
        _ = cancelled => {
            let _ = run_on_main_thread(move || cancel(id)).await;
            Ok(AuthSessionOutcome::Cancelled)
        }
    }
}

/// Shows session `id` over the presenting window. Main thread.
fn start(id: u64, url: String, scheme: String, ephemeral: bool, answer: Answer) {
    let fail = |answer: Answer, message: String| {
        let _ = answer.send(Err(AuthSessionError::failed(message)));
    };
    let mtm = MainThreadMarker::new().expect("run_on_main_thread runs on the main thread");
    let Some(window) = nsh_platform_ios::presenting_controller(mtm)
        .and_then(|top| top.viewIfLoaded())
        .and_then(|view| view.window())
    else {
        return fail(
            answer,
            "openAuthSession: there is no window to show the page over".into(),
        );
    };
    let Some(page) = NSURL::URLWithString(&NSString::from_str(&url)) else {
        return fail(answer, format!("openAuthSession: not a URL ({url})"));
    };
    // A later session replaced this one: close the earlier sheet. Its
    // completion still comes, to a session no one waits for.
    cancel_running();
    let answer = Rc::new(RefCell::new(Some(answer)));
    let pending = answer.clone();
    let completion = RcBlock::new(move |callback: *mut NSURL, error: *mut NSError| {
        // SAFETY: the completion gets a URL or an error, each null or valid
        // for the call.
        let (callback, error) = unsafe { (callback.as_ref(), error.as_ref()) };
        let result = match (callback, error) {
            (Some(callback), _) => Ok(match callback.absoluteString() {
                Some(url) => AuthSessionOutcome::Completed(url.to_string()),
                None => AuthSessionOutcome::Cancelled,
            }),
            (None, Some(error)) if user_cancelled(error) => Ok(AuthSessionOutcome::Cancelled),
            (None, Some(error)) => Err(AuthSessionError::failed(format!(
                "openAuthSession: {}",
                error.localizedDescription()
            ))),
            (None, None) => Ok(AuthSessionOutcome::Cancelled),
        };
        CURRENT.with_borrow_mut(|current| {
            if current.as_ref().is_some_and(|r| r.id == id) {
                *current = None;
            }
        });
        if let Some(answer) = answer.borrow_mut().take() {
            let _ = answer.send(result);
        }
    });
    let scheme = NSString::from_str(&scheme);
    // SAFETY: an http(s) URL (the request was checked), a scheme, and a
    // block of the completion's signature, which the session copies.
    // `initWithURL:callback:completionHandler:` needs iOS 17.4; the scheme
    // initializer works on every iOS NSH supports.
    #[allow(deprecated)]
    let session = unsafe {
        ASWebAuthenticationSession::initWithURL_callbackURLScheme_completionHandler(
            ASWebAuthenticationSession::alloc(),
            &page,
            Some(&scheme),
            RcBlock::as_ptr(&completion),
        )
    };
    let provider = Provider::alloc(mtm).set_ivars(ProviderIvars { window });
    // SAFETY: `NSObject`'s `init` on a provider just allocated.
    let provider: Retained<Provider> = unsafe { msg_send![super(provider), init] };
    // SAFETY: setters of a session not started yet; the provider lives as
    // long as the session (`Running`).
    let started = unsafe {
        session.setPresentationContextProvider(Some(ProtocolObject::from_ref(&*provider)));
        session.setPrefersEphemeralWebBrowserSession(ephemeral);
        session.start()
    };
    if !started {
        if let Some(answer) = pending.borrow_mut().take() {
            fail(
                answer,
                "openAuthSession: the sign-in page could not be shown".into(),
            );
        }
        return;
    }
    CURRENT.with_borrow_mut(|current| {
        *current = Some(Running {
            id,
            session,
            _provider: provider,
        })
    });
}

/// Closes session `id` if it still shows. Main thread.
fn cancel(id: u64) {
    let running = CURRENT.with_borrow_mut(|current| {
        if current.as_ref().is_some_and(|r| r.id == id) {
            current.take()
        } else {
            None
        }
    });
    if let Some(running) = running {
        // SAFETY: a session this crate started, on the main thread.
        unsafe { running.session.cancel() };
    }
}

/// Closes whatever session shows. Main thread.
fn cancel_running() {
    if let Some(running) = CURRENT.with_borrow_mut(Option::take) {
        // SAFETY: a session this crate started, on the main thread.
        unsafe { running.session.cancel() };
    }
}

/// Whether `error` is the user's cancellation: the page closed, or the alert
/// asking to sign in declined.
fn user_cancelled(error: &NSError) -> bool {
    // SAFETY: a constant the framework defines.
    let domain = unsafe { ASWebAuthenticationSessionErrorDomain };
    &*error.domain() == domain
        && error.code() == ASWebAuthenticationSessionErrorCode::CanceledLogin.0
}
