//! The browser flow of the desktops and Android, run headless: the page
//! opens in the browser, and a link of the callback scheme ends the session.

use std::sync::{Mutex, MutexGuard};

use nsh_core::{element::Root, prelude::*};
use nsh_engine::EngineConfig;
use nsh_oauth2::{AuthSessionOutcome, AuthSessionRequest, close_auth_session, open_auth_session};
use nsh_testing::Headless;

/// Opens a session ending with a `demo:` callback and shows how it ended;
/// listens to the app's links all along, as an app handling deep links
/// does.
#[component]
fn SigningIn() -> impl IntoElement {
    let result = signal(String::from("nothing"));
    let heard = signal(String::from("app heard"));
    spawn(async move {
        loop {
            let url = next_url_scheme_open().await;
            heard.set(format!("{} {url}", untracked(|| heard.get())));
        }
    });
    let sign_in = move |_: &()| {
        spawn(async move {
            let request = AuthSessionRequest::new("https://example.com/authorize", "demo");
            result.set(match open_auth_session(request).await {
                Ok(AuthSessionOutcome::Completed(url)) => format!("signed in {url}"),
                Ok(_) => "cancelled".into(),
                Err(error) => error.code.unwrap_or(error.message),
            })
        });
    };
    view()
        .child(text(move || result.get()))
        .child(text(move || heard.get()))
        .child(text("Sign in").on_press(sign_in))
        .child(text("Close").on_press(|_: &()| close_auth_session()))
}

/// The waiting session is process-wide, so the tests take turns: a test
/// signing in while another one waits would cancel the other's session.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn start() -> (Headless, MutexGuard<'static, ()>) {
    let turn = ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = Root::of::<SigningIn>().url_schemes(["demo", "other"]);
    let mut app = Headless::start(EngineConfig::new(root)).unwrap();
    app.first_render().unwrap();
    (app, turn)
}

#[test]
fn the_callback_link_ends_the_session() {
    let (mut app, _turn) = start();
    app.press("Sign in").unwrap();
    app.wait_for("text:nothing").unwrap();
    assert_eq!(
        app.screen().opened_urls(),
        ["https://example.com/authorize"]
    );
    app.open_with_url("demo://done?code=8").unwrap();
    app.wait_for("text:signed in demo://done?code=8").unwrap();
}

#[test]
fn the_app_keeps_hearing_its_links_during_and_after_a_session() {
    let (mut app, _turn) = start();
    app.press("Sign in").unwrap();
    app.wait_for("text:nothing").unwrap();
    // A link of another scheme is the app's while the session waits.
    app.open_with_url("other://orders/1").unwrap();
    app.wait_for("text:app heard other://orders/1").unwrap();
    // The callback is the session's alone.
    app.open_with_url("demo://done?code=7").unwrap();
    app.wait_for("text:signed in demo://done?code=7").unwrap();
    // After the session, the scheme's links are the app's again.
    app.open_with_url("demo://orders/2").unwrap();
    app.wait_for("text:app heard other://orders/1 demo://orders/2")
        .unwrap();
}

#[test]
fn a_new_session_or_closing_cancels_the_waiting_one() {
    let (mut app, _turn) = start();
    app.press("Sign in").unwrap();
    app.press("Sign in").unwrap();
    app.wait_for("text:cancelled").unwrap();
    // The second one still waits for its callback.
    app.open_with_url("demo://done?code=9").unwrap();
    app.wait_for("text:signed in demo://done?code=9").unwrap();

    app.press("Sign in").unwrap();
    app.press("Close").unwrap();
    app.wait_for("text:cancelled").unwrap();
}

#[test]
fn a_browser_that_does_not_open_fails_the_session() {
    let (mut app, _turn) = start();
    app.fail_open_urls("https://example.com/authorize").unwrap();
    app.press("Sign in").unwrap();
    app.wait_for("text:openAuthSession: the browser did not open the sign-in page")
        .unwrap();
}
