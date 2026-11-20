use crate::database::ConnectionPool;
use axum::{Router, middleware, routing::get};
use oauth2::{AuthUrl, ClientId, ClientSecret, EndpointNotSet, EndpointSet, RedirectUrl, TokenUrl};
use std::env;
use tower_http::services::ServeDir;

mod auth;
mod database;
mod error;

type BasicClient = oauth2::basic::BasicClient<
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointSet,
>;

#[derive(Clone)]
struct Ctx {
    google: BasicClient,
    github: BasicClient,
    prod: bool,
    reqwest: reqwest::Client,
    redis: ConnectionPool,
    app_url: String,
    landing_url: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let app_url = env::var("APP_URL").unwrap_or_else(|_| "http://127.0.0.1:8080".to_owned());
    let prod = env::var("ENVIRONMENT").unwrap_or_else(|_| "development".to_owned()) == "production";
    let landing_url =
        env::var("LANDING_URL").unwrap_or_else(|_| "http://127.0.0.1:8080/landing".to_owned());

    let google = oauth_client(
        env::var("GOOGLE_CLIENT_ID")?,
        env::var("GOOGLE_CLIENT_SECRET")?,
        "https://accounts.google.com/o/oauth2/v2/auth".to_owned(),
        "https://oauth2.googleapis.com/token".to_owned(),
        format!("{app_url}/auth/google/callback"),
    )?;

    let github = oauth_client(
        env::var("GITHUB_CLIENT_ID")?,
        env::var("GITHUB_CLIENT_SECRET")?,
        "https://github.com/login/oauth/authorize".to_owned(),
        "https://github.com/login/oauth/access_token".to_owned(),
        format!("{app_url}/auth/github/callback"),
    )?;

    let reqwest = reqwest::Client::builder()
        .user_agent("voulr/0.1.0")
        .redirect(reqwest::redirect::Policy::none())
        .build()?;

    let redis = bb8::Pool::builder()
        .build(redis::Client::open(env::var("REDIS_URL")?)?)
        .await?;

    let ctx = Ctx {
        google,
        github,
        prod,
        reqwest,
        redis,
        app_url,
        landing_url,
    };

    let protected = Router::new()
        .route("/", get(|| async { "hello, world!" }))
        .route_layer(middleware::from_fn_with_state(
            ctx.clone(),
            auth::middleware,
        ));

    let mut app = Router::new()
        .nest("/auth", auth::mount())
        .merge(protected)
        .with_state(ctx);

    if !prod {
        app = app.nest_service("/landing", ServeDir::new("landing"));
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:8080").await?;
    println!("listening on http://{}", listener.local_addr()?);
    Ok(axum::serve(listener, app).await?)
}

fn oauth_client(
    client_id: String,
    client_secret: String,
    auth_url: String,
    token_url: String,
    redirect_url: String,
) -> anyhow::Result<BasicClient> {
    Ok(oauth2::basic::BasicClient::new(ClientId::new(client_id))
        .set_client_secret(ClientSecret::new(client_secret))
        .set_auth_uri(AuthUrl::new(auth_url)?)
        .set_token_uri(TokenUrl::new(token_url)?)
        .set_redirect_uri(RedirectUrl::new(redirect_url)?))
}
