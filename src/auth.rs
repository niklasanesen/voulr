use crate::{Ctx, database::DatabaseConnection, error::AppError};
use anyhow::Context;
use axum::{
    Router,
    extract::{Query, Request, State},
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use axum_extra::extract::{
    CookieJar,
    cookie::{Cookie, SameSite},
};
use oauth2::{AuthorizationCode, CsrfToken, Scope, TokenResponse};
use redis::{AsyncCommands, aio::MultiplexedConnection};
use serde::Deserialize;
use uuid::Uuid;

const CSRF_TOKEN: &str = "csrf_token";
const CSRF_TOKEN_MAX_AGE: i64 = 600;
const NEXT: &str = "next";
const SESSION_TTL: u64 = 60 * 60 * 24 * 7;
const SESSION: &str = "session";

pub fn mount() -> Router<Ctx> {
    Router::new()
        .route("/google", get(google))
        .route("/github", get(github))
        .route("/google/callback", get(google_callback))
        .route("/github/callback", get(github_callback))
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

#[derive(Deserialize)]
struct CallbackParams {
    code: String,
    state: String,
}

#[derive(Deserialize)]
struct GoogleUser {
    sub: String,
    email: String,
    email_verified: bool,
}

async fn google_callback(
    jar: CookieJar,
    Query(CallbackParams { code, state }): Query<CallbackParams>,
    State(Ctx {
        google,
        reqwest,
        app_url,
        prod,
        ..
    }): State<Ctx>,
    DatabaseConnection(mut db_conn): DatabaseConnection,
) -> Result<impl IntoResponse, AppError> {
    check_csrf_token(&jar, &state)?;

    let token = google
        .exchange_code(AuthorizationCode::new(code))
        .request_async(&reqwest)
        .await
        .context("failed to send request to google authorization server")?;

    let user: GoogleUser = reqwest
        .get("https://openidconnect.googleapis.com/v1/userinfo")
        .bearer_auth(token.access_token().secret())
        .send()
        .await
        .context("failed to send request to google user api")?
        .json()
        .await
        .context("failed to deserialize the google user api response body as json")?;

    if !user.email_verified {
        return Err(anyhow::anyhow!("unverified google email").into());
    }

    let user_id = upsert_user(&mut db_conn, format!("google:{}", user.sub), user.email).await?;

    let next_url = get_next_url(&jar, &app_url);

    Ok((
        set_session(&mut db_conn, user_id, jar, prod, "/auth/google").await?,
        Redirect::to(&next_url),
    ))
}

#[derive(Deserialize)]
struct GithubUser {
    id: i64,
}

#[derive(Deserialize)]
struct GithubEmail {
    email: String,
    primary: bool,
    verified: bool,
}

async fn github_callback(
    jar: CookieJar,
    Query(CallbackParams { code, state }): Query<CallbackParams>,
    State(Ctx {
        github,
        reqwest,
        app_url,
        prod,
        ..
    }): State<Ctx>,
    DatabaseConnection(mut db_conn): DatabaseConnection,
) -> Result<impl IntoResponse, AppError> {
    check_csrf_token(&jar, &state)?;

    let token = github
        .exchange_code(AuthorizationCode::new(code))
        .request_async(&reqwest)
        .await
        .context("failed to send request to github authorization server")?;
    let access_token = token.access_token().secret();

    let user: GithubUser = reqwest
        .get("https://api.github.com/user")
        .bearer_auth(access_token)
        .send()
        .await
        .context("failed to send request to github user api")?
        .json()
        .await
        .context("failed to deserialize the github user api response body as json")?;

    let email = reqwest
        .get("https://api.github.com/user/emails")
        .bearer_auth(access_token)
        .send()
        .await
        .context("failed to send request to github emails api")?
        .json::<Vec<GithubEmail>>()
        .await
        .context("failed to deserialize the github emails api response body as json")?
        .into_iter()
        .find(|e| e.primary && e.verified)
        .map(|e| e.email)
        .context("unverified github primary email")?;

    let user_id = upsert_user(&mut db_conn, format!("github:{}", user.id), email).await?;

    let next_url = get_next_url(&jar, &app_url);

    Ok((
        set_session(&mut db_conn, user_id, jar, prod, "/auth/github").await?,
        Redirect::to(&next_url),
    ))
}

fn check_csrf_token(jar: &CookieJar, state: &str) -> anyhow::Result<()> {
    let stored_state = jar.get(CSRF_TOKEN).context("missing csrf token")?.value();
    anyhow::ensure!(stored_state == state, "csrf token mismatch");
    Ok(())
}

async fn upsert_user(
    db_conn: &mut MultiplexedConnection,
    provider_key: String,
    email: String,
) -> anyhow::Result<Uuid> {
    let stored_user_id: Option<Uuid> = db_conn.get(&provider_key).await?;
    let user_id = stored_user_id.unwrap_or_else(Uuid::now_v7);
    if stored_user_id.is_none() {
        db_conn.set::<_, _, ()>(provider_key, user_id).await?;
    }
    db_conn
        .set::<_, _, ()>(format!("user:{user_id}"), email)
        .await?;
    Ok(user_id)
}

fn get_next_url(jar: &CookieJar, app_url: &str) -> String {
    let path = jar
        .get(NEXT)
        .map(|c| c.value())
        .filter(|p| p.starts_with('/'))
        .unwrap_or("/");
    format!("{app_url}{path}")
}

async fn set_session(
    db_conn: &mut MultiplexedConnection,
    user_id: Uuid,
    jar: CookieJar,
    prod: bool,
    path: &'static str,
) -> anyhow::Result<CookieJar> {
    let session_id = Uuid::new_v4();
    db_conn
        .set_ex::<_, _, ()>(format!("session:{session_id}"), user_id, SESSION_TTL)
        .await?;
    Ok(jar
        .add(base_cookie(
            SESSION,
            session_id.to_string(),
            "/",
            prod,
            SESSION_TTL as i64,
        ))
        .remove(Cookie::build(CSRF_TOKEN).path(path))
        .remove(Cookie::build(NEXT).path("/auth")))
}

#[derive(Clone)]
pub struct UserId(pub Uuid);

pub async fn middleware(
    jar: CookieJar,
    DatabaseConnection(mut db_conn): DatabaseConnection,
    State(Ctx {
        prod, landing_url, ..
    }): State<Ctx>,
    mut req: Request,
    next: Next,
) -> Result<Response, AppError> {
    if let Some(session) = jar.get(SESSION)
        && let Ok(session_id) = session.value().parse::<Uuid>()
        && let Some(user_id) = db_conn.get(format!("session:{session_id}")).await?
    {
        req.extensions_mut().insert(UserId(user_id));
        return Ok(next.run(req).await);
    }
    let path = req
        .uri()
        .path_and_query()
        .map_or("/", |p| p.as_str())
        .to_owned();
    Ok((
        jar.add(base_cookie(NEXT, path, "/auth", prod, 3600)),
        Redirect::to(&format!("{landing_url}/login.html")),
    )
        .into_response())
}
