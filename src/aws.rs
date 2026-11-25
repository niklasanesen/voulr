use crate::{auth::UserId, database::DatabaseConnection, error::AppError};
use axum::{
    Extension,
    response::{IntoResponse, Redirect},
};
use serde::Serialize;
use uuid::Uuid;

#[derive(Serialize)]
struct AwsConnection {
    owner: Uuid,
    role_arn: Option<String>,
}

pub async fn connect(
    Extension(UserId(user_id)): Extension<UserId>,
    DatabaseConnection(mut db_conn): DatabaseConnection,
) -> Result<impl IntoResponse, AppError> {
    let external_id = Uuid::new_v4();
    let pending = AwsConnection {
        owner: user_id,
        role_arn: None,
    };
    redis::pipe()
        .atomic()
        .set_ex(
            format!("conn:{external_id}"),
            serde_json::to_string(&pending)?,
            3600,
        )
        .sadd(format!("user:{user_id}:conns"), external_id)
        .query_async::<()>(&mut *db_conn)
        .await?;
    Ok(Redirect::to(&format!(
        "https://eu-west-1.console.aws.amazon.com/cloudformation/home?region=eu-west-1#/stacks/create/review?templateURL=https://voulr-bucket.s3.eu-west-1.amazonaws.com/connect-aws.json&stackName=voulr-stack-{external_id}&param_ExternalId={external_id}",
    )))
}
