# nsh-oauth2

OAuth 2 sign-in for [Native Shell (NSH)](https://github.com/longbridge/nsh) apps, as an [nshm](https://longbridge.github.io/nsh/docs/nshm).

Your app opens the provider's sign-in page. The user signs in. The page redirects to a URL of your app's own scheme, such as `myapp://signed-in?code=...`, and that URL comes back to your app. You then exchange the code for tokens with your own HTTP client. This crate only shows the page and brings the callback back; it does not talk to the provider.

| Platform | How the page shows | How the callback comes back |
|---|---|---|
| iOS | `ASWebAuthenticationSession`, a sheet over the app | From the system. The scheme needs no entry in `nsh.json`. |
| Android | A Custom Tab (the default browser if it has none) | As a link of a scheme the app claims (`nsh.json` `schemes`). Coming back to the app without one cancels the session. |
| macOS, Windows, Linux | The system browser | As a link of a scheme the app claims. Nothing tells the app the user gave up, so the session waits until the callback, `closeAuthSession()` or a new session. |
| Web | - | Fails at once with `ERR_FEATURE_UNAVAILABLE_ON_PLATFORM`: a web page hears no links of the app's schemes. |

One session runs at a time. Opening a new one ends the earlier one as cancelled.

## Install

```sh
nsh add github:longbridge/nsh-oauth2
```

On Android and the desktops, claim the callback scheme in `nsh.json`:

```json
{
  "schemes": ["myapp"]
}
```

## TSX

```tsx
import { OAuth2 } from "nsh-oauth2";

const url = await OAuth2.openAuthSession({
  url: "https://accounts.example.com/authorize?client_id=...&redirect_uri=myapp://signed-in&response_type=code&code_challenge=...",
  callbackScheme: "myapp",
  ephemeral: false, // true: share no cookies with the user's browser
});

if (url === null) {
  // The user closed the page, or the app closed the session.
} else {
  const code = new URL(url).searchParams.get("code");
  // Exchange `code` for tokens with fetch().
}

// Close the session while it waits: openAuthSession resolves with null.
await OAuth2.closeAuthSession();
```

`openAuthSession` rejects with an `Error` when the URL or scheme is wrong, and with the code `ERR_FEATURE_UNAVAILABLE_ON_PLATFORM` on the web.

## Rust

```rust
use nsh_core::prelude::*;
use nsh_oauth2::{AuthSessionOutcome, AuthSessionRequest};

// In a task:
let request = AuthSessionRequest::new(authorize_url, "myapp").ephemeral(true);
match nsh_oauth2::open_auth_session(request).await {
    Ok(AuthSessionOutcome::Completed(url)) => exchange_code(url),
    Ok(_) => {} // cancelled
    Err(error) => log::warn!("sign-in failed: {error} ({:?})", error.code),
}

// Anywhere: ends the waiting session as cancelled.
nsh_oauth2::close_auth_session();
```

## Things to know

- Use PKCE (`code_challenge`) and a `state` value, and check the `state` in the callback. The callback URL is untrusted input.
- On Android and the desktops, while a session waits this crate listens for the app's links (`next_url_scheme_open`) and takes the callback. A Rust app that awaits `next_url_scheme_open` itself should not open a session at the same time, or one of them misses links.
- The GPUI and terminal backends get no links of the app's schemes, so a session there opens the browser but its callback never arrives.

## Moving from NSH core

NSH up to 0.7 had this feature built in as `nsh_core::element::open_auth_session` and `close_auth_session`. It is deprecated there and will be removed. To move:

1. `nsh add github:longbridge/nsh-oauth2`.
2. Replace `nsh_core::auth_session::AuthSessionRequest` and friends with `nsh_oauth2::AuthSessionRequest`, and `open_auth_session` / `close_auth_session` with the functions of the same name in `nsh_oauth2`.

The request, outcome and error types have the same fields as before.

## License

Apache-2.0, as NSH.
