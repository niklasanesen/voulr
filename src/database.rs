use crate::{Ctx, error::AppError};
use axum::{extract::FromRequestParts, http::request::Parts};

pub type ConnectionPool = bb8::Pool<redis::Client>;
pub struct DatabaseConnection(pub bb8::PooledConnection<'static, redis::Client>);

impl FromRequestParts<Ctx> for DatabaseConnection {
    type Rejection = AppError;

    async fn from_request_parts(_parts: &mut Parts, ctx: &Ctx) -> Result<Self, Self::Rejection> {
        Ok(Self(ctx.redis.get_owned().await?))
    }
}
