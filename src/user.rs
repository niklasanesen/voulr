use crate::{Ctx, auth::UserId, database::DatabaseConnection, error::AppError};
use anyhow::Context;
use axum::{Extension, extract::FromRequestParts, http::request::Parts};
use redis::AsyncCommands;

pub struct UserEmail(pub String);

impl FromRequestParts<Ctx> for UserEmail {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, ctx: &Ctx) -> Result<Self, Self::Rejection> {
        let Extension(UserId(user_id)) =
            Extension::<UserId>::from_request_parts(parts, ctx).await?;
        let DatabaseConnection(mut db_conn) =
            DatabaseConnection::from_request_parts(parts, ctx).await?;
        let email = db_conn
            .get::<_, Option<String>>(format!("user:{user_id}"))
            .await?
            .context("missing user from session")?;
        Ok(UserEmail(email))
    }
}
