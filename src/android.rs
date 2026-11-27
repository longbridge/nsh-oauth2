//! Android: the sign-in page in a Custom Tab (the default browser where it
//! has none), started for a result from the activity in front
//! (`nsh::activity`). The page's redirect to the app's callback scheme comes
//! back as a link of a scheme the app claims, which ends the session. The
//! tab's result comes when the user is back in the app (the back button,
//! closing the tab, or the callback bringing the app to the front); a
//! callback may arrive just after it, so the session waits a moment for one
//! before it ends as cancelled.

use std::{future::Future, pin::Pin, time::Duration};

use nsh::{
    activity::{self, ActivityError, ActivityResult},
    jni::{
        JNIEnv,
        objects::{JObject, JValue},
    },
};
use nsh_core::prelude::*;
use tokio::sync::oneshot;

use crate::{
    AuthSessionError, AuthSessionOutcome, AuthSessionRequest, expect_callback, wait_for_callback,
};

/// Makes an `ACTION_VIEW` intent a Custom Tab (the Custom Tabs protocol; no
/// session binder).
const EXTRA_SESSION: &str = "android.support.customtabs.extra.SESSION";
/// Asks a browser for ephemeral browsing in the tab, where it offers it.
const EXTRA_EPHEMERAL: &str = "androidx.browser.customtabs.extra.ENABLE_EPHEMERAL_BROWSING";
/// How long a callback may come after the tab's result.
const LATE_CALLBACK: Duration = Duration::from_millis(1500);

type Started = Pin<Box<dyn Future<Output = Result<ActivityResult, ActivityError>> + Send>>;

pub(crate) async fn run(
    _id: u64,
    request: &AuthSessionRequest,
    mut cancelled: oneshot::Receiver<()>,
) -> Result<AuthSessionOutcome, AuthSessionError> {
    let (url, ephemeral) = (request.url.clone(), request.ephemeral);
    let mut callback = expect_callback(request);
    let mut tab = nsh::with_env(|env| Ok(start(env, &url, ephemeral))).map_err(|error| {
        AuthSessionError::failed(format!("openAuthSession: no Java environment: {error}"))
    })??;
    tokio::select! {
        outcome = wait_for_callback(&mut callback, &mut cancelled) => outcome,
        _ = &mut tab => {
            // Back in the app: a callback that brought it back may still be
            // on its way.
            tokio::select! {
                outcome = wait_for_callback(&mut callback, &mut cancelled) => outcome,
                _ = timer(LATE_CALLBACK) => Ok(AuthSessionOutcome::Cancelled),
            }
        }
    }
}

/// Starts the Custom Tab showing `url`: its result to come, or why it cannot
/// start.
fn start(env: &mut JNIEnv<'_>, url: &str, ephemeral: bool) -> Result<Started, AuthSessionError> {
    let failed = |error: nsh::jni::errors::Error| {
        AuthSessionError::failed(format!(
            "openAuthSession: the browser cannot be started: {error}"
        ))
    };
    let intent = tab_intent(env, url, ephemeral).map_err(failed)?;
    if !activity::can_start(env, &intent) {
        return Err(AuthSessionError::unavailable(
            "openAuthSession: no browser shows the sign-in page",
        ));
    }
    Ok(Box::pin(activity::start_for_result(env, &intent)))
}

/// `new Intent(ACTION_VIEW, Uri.parse(url))`, browsable, with the Custom
/// Tabs session extra (and ephemeral browsing when asked).
fn tab_intent<'local>(
    env: &mut JNIEnv<'local>,
    url: &str,
    ephemeral: bool,
) -> nsh::jni::errors::Result<JObject<'local>> {
    let action = env.new_string("android.intent.action.VIEW")?;
    let text = env.new_string(url)?;
    let uri = env
        .call_static_method(
            "android/net/Uri",
            "parse",
            "(Ljava/lang/String;)Landroid/net/Uri;",
            &[JValue::Object(&text)],
        )?
        .l()?;
    let intent = env.new_object(
        "android/content/Intent",
        "(Ljava/lang/String;Landroid/net/Uri;)V",
        &[JValue::Object(&action), JValue::Object(&uri)],
    )?;
    let browsable = env.new_string("android.intent.category.BROWSABLE")?;
    env.call_method(
        &intent,
        "addCategory",
        "(Ljava/lang/String;)Landroid/content/Intent;",
        &[JValue::Object(&browsable)],
    )?;
    let extras = env.new_object("android/os/Bundle", "()V", &[])?;
    let session = env.new_string(EXTRA_SESSION)?;
    env.call_method(
        &extras,
        "putBinder",
        "(Ljava/lang/String;Landroid/os/IBinder;)V",
        &[JValue::Object(&session), JValue::Object(&JObject::null())],
    )?;
    env.call_method(
        &intent,
        "putExtras",
        "(Landroid/os/Bundle;)Landroid/content/Intent;",
        &[JValue::Object(&extras)],
    )?;
    if ephemeral {
        let key = env.new_string(EXTRA_EPHEMERAL)?;
        env.call_method(
            &intent,
            "putExtra",
            "(Ljava/lang/String;Z)Landroid/content/Intent;",
            &[JValue::Object(&key), JValue::Bool(1)],
        )?;
    }
    Ok(intent)
}
