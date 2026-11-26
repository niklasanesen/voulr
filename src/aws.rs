use crate::{Ctx, auth::UserId, database::DatabaseConnection, error::AppError};
use anyhow::Context;
use axum::{
    Extension, Json,
    extract::State,
    response::{IntoResponse, Redirect},
};
use redis::{AsyncCommands, aio::MultiplexedConnection};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Serialize, Deserialize)]
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

#[derive(Deserialize)]
enum ReqType {
    Create,
    Update,
    Delete,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Props {
    external_id: Uuid,
    role_arn: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CfReq {
    request_type: ReqType,
    request_id: String,
    stack_id: String,
    #[serde(rename = "ResponseURL")]
    response_url: String,
    logical_resource_id: String,
    resource_properties: Props,
}

#[derive(Serialize)]
#[serde(rename_all = "UPPERCASE")]
enum Status {
    Success,
    Failed,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct CfRes {
    status: Status,
    request_id: String,
    stack_id: String,
    logical_resource_id: String,
    physical_resource_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

pub async fn events(
    DatabaseConnection(mut db_conn): DatabaseConnection,
    State(Ctx { reqwest, .. }): State<Ctx>,
    Json(CfReq {
        request_type,
        request_id,
        stack_id,
        response_url,
        logical_resource_id,
        resource_properties,
    }): Json<CfReq>,
) -> Result<(), AppError> {
    let (status, reason) =
        match events_helper(&resource_properties, &mut db_conn, request_type).await {
            Ok(()) => (Status::Success, None),
            Err(e) => (Status::Failed, Some(e.to_string())),
        };
    let body = CfRes {
        status,
        request_id,
        stack_id,
        logical_resource_id,
        physical_resource_id: resource_properties.external_id.to_string(),
        reason,
    };
    reqwest
        .put(response_url)
        .json(&body)
        .send()
        .await
        .context("failed to send request to cloudformation")?
        .error_for_status()
        .context("cloudformation response rejected")?;
    Ok(())
}

async fn events_helper(
    Props {
        external_id,
        role_arn,
    }: &Props,
    db_conn: &mut MultiplexedConnection,
    request_type: ReqType,
) -> anyhow::Result<()> {
    let aws_conn_key = format!("conn:{external_id}");
    let aws_conn_exists: Option<String> = db_conn.get(&aws_conn_key).await?;
    match request_type {
        ReqType::Create | ReqType::Update => {
            let mut aws_conn: AwsConnection = serde_json::from_str(
                &aws_conn_exists.context("unknown or expired aws connection")?,
            )?;
            if let Some(stored_role_arn) = &aws_conn.role_arn {
                anyhow::ensure!(stored_role_arn == role_arn, "aws connection already in use");
                return Ok(());
            }
            aws_conn.role_arn = Some(role_arn.clone());
            db_conn
                .set::<_, _, ()>(aws_conn_key, serde_json::to_string(&aws_conn)?)
                .await?;
        }
        ReqType::Delete => {
            if let Some(aws_conn_json) = aws_conn_exists {
                let aws_conn: AwsConnection = serde_json::from_str(&aws_conn_json)?;
                redis::pipe()
                    .atomic()
                    .del(&aws_conn_key)
                    .srem(format!("user:{}:conns", aws_conn.owner), external_id)
                    .query_async::<()>(db_conn)
                    .await?;
            }
        }
    };
    Ok(())
}
