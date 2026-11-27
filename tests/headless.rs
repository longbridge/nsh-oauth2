//! The browser flow of the desktops and Android, run headless: the page
//! opens in the browser, and a link of the callback scheme ends the session.

use nsh_core::{element::Root, prelude::*};
use nsh_engine::EngineConfig;
use nsh_oauth2::{AuthSessionOutcome, AuthSessionRequest, close_auth_session, open_auth_session};
use nsh_testing::Headless;

/// Opens a session ending with a `demo:` callback and shows how it ended.
#[component]
fn SigningIn() -> impl IntoElement {
    let result = signal(String::from("nothing"));
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
        .child(text("Sign in").on_press(sign_in))
        .child(text("Close").on_press(|_: &()| close_auth_session()))
}

fn start() -> Headless {
    let root = Root::of::<SigningIn>().url_schemes(["demo"]);
    let mut app = Headless::start(EngineConfig::new(root)).unwrap();
    app.first_render().unwrap();
    app
}

#[test]
fn the_callback_link_ends_the_session() {
    let mut app = start();
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
fn a_new_session_or_closing_cancels_the_waiting_one() {
    let mut app = start();
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
    let mut app = start();
    app.fail_open_urls("https://example.com/authorize").unwrap();
    app.press("Sign in").unwrap();
    app.wait_for("text:openAuthSession: the browser did not open the sign-in page")
        .unwrap();
}
