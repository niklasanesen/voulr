use crate::{Ctx, error::AppError, user::UserEmail};
use anyhow::Context;
use axum::{
    extract::{FromRequestParts, OriginalUri},
    http::request::Parts,
};

pub struct Layout {
    pub landing_url: String,
    pub initial: char,
    pub path: String,
}

impl FromRequestParts<Ctx> for Layout {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, ctx: &Ctx) -> Result<Self, Self::Rejection> {
        let UserEmail(email) = UserEmail::from_request_parts(parts, ctx).await?;
        let OriginalUri(uri) = OriginalUri::from_request_parts(parts, ctx).await?;
        Ok(Layout {
            landing_url: ctx.landing_url.clone(),
            initial: email.chars().next().context("empty user email")?,
            path: uri.path().to_owned(),
        })
    }
}
