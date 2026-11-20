use crate::Ctx;
use axum::{
    Router,
    extract::State,
    response::{IntoResponse, Redirect},
    routing::get,
};
use axum_extra::extract::{
    CookieJar,
    cookie::{Cookie, SameSite},
};
use oauth2::{CsrfToken, Scope};

const CSRF_TOKEN: &str = "csrf_token";
const CSRF_TOKEN_MAX_AGE: i64 = 600;

pub fn mount() -> Router<Ctx> {
    Router::new()
        .route("/google", get(google))
        .route("/github", get(github))
}

async fn google(State(Ctx { google, prod, .. }): State<Ctx>, jar: CookieJar) -> impl IntoResponse {
    let (auth_url, csrf_token) = google
        .authorize_url(CsrfToken::new_random)
        .add_scope(Scope::new("openid".to_owned()))
        .add_scope(Scope::new("email".to_owned()))
        .url();
    (
        jar.add(base_cookie(
            CSRF_TOKEN,
            csrf_token.into_secret(),
            "/auth/google",
            prod,
            CSRF_TOKEN_MAX_AGE,
        )),
        Redirect::to(auth_url.as_ref()),
    )
}

async fn github(State(Ctx { github, prod, .. }): State<Ctx>, jar: CookieJar) -> impl IntoResponse {
    let (auth_url, csrf_token) = github
        .authorize_url(CsrfToken::new_random)
        .add_scope(Scope::new("user:email".to_owned()))
        .url();
    (
        jar.add(base_cookie(
            CSRF_TOKEN,
            csrf_token.into_secret(),
            "/auth/github",
            prod,
            CSRF_TOKEN_MAX_AGE,
        )),
        Redirect::to(auth_url.as_ref()),
    )
}

fn base_cookie(
    name: &'static str,
    value: String,
    path: &'static str,
    prod: bool,
    max_age: i64,
) -> Cookie<'static> {
    Cookie::build((name, value))
        .path(path)
        .http_only(true)
        .secure(prod)
        .same_site(SameSite::Lax)
        .max_age(time::Duration::seconds(max_age))
        .build()
}
