# nsh-oauth2

Let users sign in to your [Native Shell (NSH)](https://github.com/longbridge/nsh) app with an OAuth 2 provider: GitHub, Google, your own identity server, anything with an authorization page.

You give it the provider's authorization URL. It shows the sign-in page the way each platform expects, waits for the provider to redirect back to your app (`myapp://signed-in?code=...`), and hands you that callback URL. You then exchange the code for tokens with `fetch` or your own HTTP client.

It works from TSX and from Rust. It is an [nshm](https://longbridge.github.io/nsh/docs/nshm), so a TSX app needs no Rust code of its own.

## What users see

| Platform | The sign-in page | When the user gives up |
|---|---|---|
| iOS | A system sign-in sheet over your app (`ASWebAuthenticationSession`) | Closing the sheet resolves with `null` |
| Android | A Custom Tab, or the default browser if it has no Custom Tabs | Coming back to the app without signing in resolves with `null` |
| macOS, Windows, Linux | The system browser | Nothing tells your app. The session waits until the callback arrives, you close it, or you start a new one |
| Web | Not supported: `openAuthSession` rejects at once with the code `ERR_FEATURE_UNAVAILABLE_ON_PLATFORM` | - |

## Install

```sh
nsh add nsh-oauth2
```

Then set up your app in `nsh.json`:

1. **Claim the callback scheme**, so Android and the desktops send the redirect back to your app. iOS does not need this, but claiming it everywhere is simplest.

   ```json
   {
     "schemes": ["myapp"]
   }
   ```

2. **If your app declares `"capabilities"`**, let it open the sign-in page: `"openUrl": { "schemes": ["https"] }` (or `"openUrl": true`). An app without `"capabilities"` can already do this.

3. **Register the redirect URI with your provider**, for example `myapp://signed-in`. It must use the scheme from step 1.

## Sign in from TSX

```tsx
import { OAuth2 } from "nsh-oauth2";

async function signIn() {
  const state = crypto.randomUUID();
  const { verifier, challenge } = await makePkce(); // PKCE: your code, or a library

  const authorize = new URL("https://accounts.example.com/authorize");
  authorize.search = new URLSearchParams({
    client_id: "YOUR_CLIENT_ID",
    redirect_uri: "myapp://signed-in",
    response_type: "code",
    scope: "openid profile",
    state,
    code_challenge: challenge,
    code_challenge_method: "S256",
  }).toString();

  let callback: string | null;
  try {
    callback = await OAuth2.openAuthSession({
      url: authorize.toString(),
      callbackScheme: "myapp",
      ephemeral: false, // true: don't share cookies with the user's browser
    });
  } catch (error) {
    // A bad URL or scheme, no browser, or the web (error.code is
    // "ERR_FEATURE_UNAVAILABLE_ON_PLATFORM").
    return;
  }
  if (callback === null) return; // The user gave up, or the session was closed.

  const params = new URL(callback).searchParams;
  if (params.get("state") !== state) return; // Not the answer to this request.

  const response = await fetch("https://accounts.example.com/token", {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      grant_type: "authorization_code",
      code: params.get("code") ?? "",
      redirect_uri: "myapp://signed-in",
      client_id: "YOUR_CLIENT_ID",
      code_verifier: verifier,
    }).toString(),
  });
  const tokens = await response.json();
  // Store tokens.access_token / tokens.refresh_token.
}
```

The token endpoint, scopes and parameters come from your provider's documentation. This crate does not talk to the provider.

### Closing the page from your app

Call `closeAuthSession()` when you no longer need the page: the user signed in some other way, or pressed a Cancel button on your desktop "Waiting for the browser" screen. The waiting `openAuthSession` resolves with `null`, and iOS takes its sheet away. A tab the desktop or Android browser opened stays open.

```tsx
await OAuth2.closeAuthSession();
```

## Sign in from Rust

`nsh add nsh-oauth2` works for Rust apps too, or add `nsh-oauth2` to your crate's `Cargo.toml`. The same functions are there:

```rust
use nsh_core::prelude::*;
use nsh_oauth2::{AuthSessionOutcome, AuthSessionRequest};

fn sign_in(authorize_url: String) {
    spawn(async move {
        let request = AuthSessionRequest::new(authorize_url, "myapp").ephemeral(true);
        match nsh_oauth2::open_auth_session(request).await {
            Ok(AuthSessionOutcome::Completed(callback)) => {
                // Check `state`, then exchange `code` for tokens.
            }
            Ok(_) => {} // The user gave up, or the session was closed.
            Err(error) => log::warn!("sign-in failed: {error} ({:?})", error.code),
        }
    });
}

// Ends the waiting session as cancelled.
nsh_oauth2::close_auth_session();
```

`error.code` is `Some(nsh_oauth2::UNAVAILABLE)` when this platform cannot show the page or get the callback back, and `None` for other failures. `AuthSessionRequest::check()` tells you up front whether a URL and scheme will be accepted.

## Good to know

- **Use PKCE and `state`.** A desktop app or a mobile app cannot keep a client secret, and any app can open a link of your scheme. Check `state` in every callback.
- **The sign-in page must be `https:` (or `http:`), and the callback scheme must be your own.** A scheme is a letter followed by letters, digits, `+`, `-` or `.`, at most 64 bytes, given without `:`. Web and local-file schemes (`https`, `file`, `javascript`, ...) are refused.
- **One sign-in at a time.** Starting a new session ends the earlier one with `null` (`Cancelled` in Rust).
- **`ephemeral: true`** asks for a private session that shares no cookies with the user's browser, so the user signs in from scratch. iOS always honors it; on Android it depends on the browser; the desktops ignore it.
- **On the desktops, a session can wait forever.** If the user closes the browser tab, your app is not told. Show a Cancel button that calls `closeAuthSession()`.
- **Your app keeps hearing its links.** On Android and the desktops, a waiting session takes only the next link of its callback scheme. Your `UrlScheme.on("open")` listeners and `next_url_scheme_open()` loops get every other link, during the session and after it. If your app may also get links of that same scheme while a session waits, give sign-in a scheme of its own (for example `myapp-auth`) and claim both in `nsh.json`.
- **GPUI and terminal backends** receive no links of the app's schemes, so a session there opens the browser but the callback never arrives.

## Moving from NSH core

NSH up to 0.7 had this built in as `open_auth_session` and `close_auth_session` in `nsh_core`. They are deprecated there and will be removed. To move:

1. Run `nsh add nsh-oauth2`.
2. Replace `nsh_core::auth_session::{AuthSessionRequest, AuthSessionOutcome, AuthSessionError}` with the same names from `nsh_oauth2`.
3. Replace calls to `open_auth_session` and `close_auth_session` with `nsh_oauth2::open_auth_session` and `nsh_oauth2::close_auth_session`. `nsh_oauth2::open_auth_session` is an `async fn`: await it inside `spawn`, as above.

The request, outcome and error types have the same fields as before.

## License

Apache-2.0, as NSH.
