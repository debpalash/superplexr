//! Optional compact workflow inspection; no execution or settlement routes.
use crate::Observer;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use superplexr_client::ClientError;
use superplexr_core::{MissionId, RunId, VerificationCatalog, VerificationStatus};

type Failure = (StatusCode, Json<serde_json::Value>);
fn failure(status: StatusCode, message: &str) -> Failure {
    (status, Json(serde_json::json!({"error":message})))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CatalogQuery {
    after: Option<RunId>,
    limit: Option<u16>,
}

pub(crate) async fn catalog(
    State(observer): State<Observer>,
    Path(mission): Path<MissionId>,
    Query(query): Query<CatalogQuery>,
) -> Result<Json<VerificationCatalog>, Failure> {
    if !observer.workflow_read {
        return Err(failure(
            StatusCode::FORBIDDEN,
            "Workflow inspection is disabled on this gateway.",
        ));
    }
    let limit = query.limit.unwrap_or(32);
    if !(1..=64).contains(&limit) {
        return Err(failure(
            StatusCode::BAD_REQUEST,
            "Page limit must be between 1 and 64.",
        ));
    }
    let permit = observer.streams.clone().try_acquire_owned().map_err(|_| {
        failure(
            StatusCode::TOO_MANY_REQUESTS,
            "Read capacity is occupied; refresh later.",
        )
    })?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        observer
            .client
            .verification_catalog(mission, query.after, limit)
            .map(Json)
            .map_err(|error| match error {
                ClientError::Remote { .. } => failure(
                    StatusCode::FORBIDDEN,
                    "This Mission is unavailable or outside this Share's scope.",
                ),
                ClientError::Io(error) if error.kind() == std::io::ErrorKind::Unsupported => {
                    failure(
                        StatusCode::NOT_IMPLEMENTED,
                        "Update the runtime to enable compact verifier discovery.",
                    )
                }
                ClientError::ReceiveLimit { .. } => failure(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "Native response limit reached; reconnect explicitly.",
                ),
                _ => failure(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Verifier discovery was not confirmed; refresh explicitly.",
                ),
            })
    })
    .await
    .map_err(|_| {
        failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "Verifier discovery stopped; refresh explicitly.",
        )
    })?
}

pub(crate) async fn status(
    State(observer): State<Observer>,
    Path((mission, verifier)): Path<(MissionId, RunId)>,
) -> Result<Json<VerificationStatus>, Failure> {
    if !observer.workflow_read {
        return Err(failure(
            StatusCode::FORBIDDEN,
            "Workflow inspection is disabled on this gateway.",
        ));
    }
    let permit = observer.streams.clone().try_acquire_owned().map_err(|_| {
        failure(
            StatusCode::TOO_MANY_REQUESTS,
            "Read capacity is occupied; refresh later.",
        )
    })?;
    tokio::task::spawn_blocking(move || {
        // Cancellation cannot free capacity while admitted native work runs.
        let _permit = permit;
        observer
            .client
            .verification_status(mission, verifier)
            .map(Json)
            .map_err(|error| match error {
                ClientError::Remote { .. } => failure(
                    StatusCode::FORBIDDEN,
                    "This verifier is unavailable or outside this Share's Mission scope.",
                ),
                ClientError::Io(error) if error.kind() == std::io::ErrorKind::Unsupported => {
                    failure(
                        StatusCode::NOT_IMPLEMENTED,
                        "Update the runtime to enable compact verification status.",
                    )
                }
                ClientError::ReceiveLimit { .. } => failure(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "Native response limit reached; reconnect explicitly.",
                ),
                _ => failure(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Workflow status was not confirmed; refresh explicitly.",
                ),
            })
    })
    .await
    .map_err(|_| {
        failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "Workflow inspection stopped; refresh explicitly.",
        )
    })?
}
