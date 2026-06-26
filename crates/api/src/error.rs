//! The error airlock. `AppError` is the only error type that crosses into an HTTP response, and it
//! is deliberately split into "safe to show the caller" 4xx variants and opaque 5xx variants whose
//! detail is logged but never wired out. This keeps internal errors (and object existence) from
//! leaking, and gives every 4xx a stable machine-readable `kind`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

#[derive(Debug)]
pub enum AppError {
    /// 422 — a well-formed request that violated a domain/value rule. `kind` is the stable wire id.
    /// `missing` carries the unmet close-checks for `close_preconditions_unmet`.
    Unprocessable {
        kind: &'static str,
        message: String,
        missing: Option<Vec<String>>,
    },
    /// 400 — a caller-fixable malformed reference (e.g. an assignee that does not exist).
    BadRequest { kind: &'static str, message: String },
    /// 404 — leak-free denial: the same shape whether a resource is absent or unreachable. `kind`
    /// is usually "not_found" but may name a specific missing reference (e.g. "unknown_workflow").
    NotFound(&'static str),
    /// 500 — the `cases_guard` trigger RAISEd (SQLSTATE WG001) during a write the engine had
    /// ALLOWED. That means the engine and the DB backstop disagreed: a bug, surfaced loudly.
    WorkflowGuard(String),
    /// 500 — any other internal failure (a sqlx error, a decode error). Detail is logged; the wire
    /// message is generic.
    Internal(String),
}

impl AppError {
    pub fn unprocessable(kind: &'static str, message: impl Into<String>) -> Self {
        AppError::Unprocessable {
            kind,
            message: message.into(),
            missing: None,
        }
    }
    pub fn bad_request(kind: &'static str, message: impl Into<String>) -> Self {
        AppError::BadRequest {
            kind,
            message: message.into(),
        }
    }
    /// The standard leak-free 404 used for an absent-or-unreachable case.
    pub fn not_found() -> Self {
        AppError::NotFound("not_found")
    }

    /// The HTTP status this error maps to.
    pub fn status(&self) -> StatusCode {
        match self {
            AppError::Unprocessable { .. } => StatusCode::UNPROCESSABLE_ENTITY,
            AppError::BadRequest { .. } => StatusCode::BAD_REQUEST,
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::WorkflowGuard(_) | AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// The caller-facing JSON body: a stable `error` kind + `message` (+ `missing` for the close
    /// gate). 5xx bodies are deliberately generic — internal detail is logged, never wired. Shared
    /// by the HTTP response and the MCP tool-error surface, so both speak the identical contract.
    pub fn to_wire(&self) -> serde_json::Value {
        match self {
            AppError::Unprocessable { kind, message, missing } => {
                let mut b = serde_json::json!({ "error": kind, "message": message });
                if let Some(m) = missing {
                    b["missing"] = serde_json::json!(m);
                }
                b
            }
            AppError::BadRequest { kind, message } => {
                serde_json::json!({ "error": kind, "message": message })
            }
            AppError::NotFound(kind) => serde_json::json!({ "error": kind, "message": "not found" }),
            AppError::WorkflowGuard(_) => {
                serde_json::json!({ "error": "workflow_guard", "message": "workflow guard violation" })
            }
            AppError::Internal(_) => serde_json::json!({ "error": "internal", "message": "internal error" }),
        }
    }
}

/// A sqlx error becomes either a `WorkflowGuard` (if it is the trigger's pinned SQLSTATE) or an
/// opaque `Internal`. Nothing from the inner error reaches the client.
impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        if let Some(db) = e.as_database_error() {
            if db.code().as_deref() == Some("WG001") {
                return AppError::WorkflowGuard(db.message().to_string());
            }
        }
        AppError::Internal(format!("{e:?}"))
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        // Log the opaque 5xx detail before discarding it; 4xx are already self-describing.
        match &self {
            AppError::WorkflowGuard(detail) => {
                tracing::error!("workflow_guard (engine/trigger drift): {detail}")
            }
            AppError::Internal(detail) => tracing::error!("internal error: {detail}"),
            _ => {}
        }
        (self.status(), Json(self.to_wire())).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status_of(e: AppError) -> StatusCode {
        e.into_response().status()
    }

    #[test]
    fn taxonomy_maps_to_expected_status_codes() {
        assert_eq!(
            status_of(AppError::unprocessable("invalid_transition", "x")),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            status_of(AppError::bad_request("invalid_assignee", "x")),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(status_of(AppError::not_found()), StatusCode::NOT_FOUND);
        assert_eq!(
            status_of(AppError::NotFound("unknown_workflow")),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status_of(AppError::WorkflowGuard("d".into())),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            status_of(AppError::Internal("d".into())),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
