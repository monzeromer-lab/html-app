//! The wire format between the page and the host (PRD §9.1).
//!
//! Three message shapes, not one. A single request/response shape would force every bulk payload
//! into one base64 blob, which the PRD calls out as the thing that makes tailing a log or reading
//! a 2 GB file fall apart. So streams are first-class here, and genuinely large transfers escape
//! JSON entirely via a host-minted `blob:` URL.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Correlates a response with its request. Minted by the page.
pub type RequestId = u64;

/// Page → host.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "t", rename_all = "camelCase")]
pub enum ClientMessage {
    /// `await htmlapp.invoke(method, params)`
    Invoke {
        id: RequestId,
        method: String,
        #[serde(default)]
        params: Value,
    },
    /// `for await (const chunk of htmlapp.stream(method, params))`
    StreamStart {
        id: RequestId,
        method: String,
        #[serde(default)]
        params: Value,
    },
    /// The consumer broke out of the loop, or called `.cancel()`.
    StreamCancel { id: RequestId },
    /// `htmlapp.on(event, handler)`
    Subscribe { id: RequestId, event: String },
    /// The unsubscribe function returned by `on` was called.
    Unsubscribe { id: RequestId },
}

/// Host → page.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "t", rename_all = "camelCase")]
pub enum HostMessage {
    /// Resolves the promise for `id`.
    Result { id: RequestId, value: Value },
    /// Rejects the promise for `id`.
    Error { id: RequestId, error: RpcError },
    /// One item from an open stream.
    Chunk { id: RequestId, value: Value },
    /// The stream completed normally.
    End { id: RequestId },
    /// The stream failed; the `for await` loop throws.
    StreamError { id: RequestId, error: RpcError },
    /// A subscribed event fired. Not correlated to a request id, because one event may have many
    /// subscribers.
    Event { event: String, payload: Value },
}

impl HostMessage {
    /// Serialise for delivery to the page.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|e| {
            // Falling back to a well-formed error keeps a serialisation bug in one API from
            // wedging the page's whole bridge.
            format!(
                r#"{{"t":"error","id":0,"error":{{"code":"internal","message":{}}}}}"#,
                serde_json::to_string(&e.to_string()).unwrap_or_else(|_| "\"?\"".into())
            )
        })
    }
}

/// A structured failure, delivered to the page as a rejected promise.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RpcError {
    pub code: ErrorCode,
    pub message: String,
    /// Extra machine-readable context — the denied path, the refused origin, and so on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    /// The refusal that the whole capability model exists to produce.
    pub fn denied(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::PermissionDenied, message)
    }

    pub fn not_found(method: &str) -> Self {
        Self::new(
            ErrorCode::MethodNotFound,
            format!("no such method: {method}"),
        )
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidParams, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unsupported, message)
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for RpcError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorCode {
    /// The manifest did not grant this. Distinct from `methodNotFound` so a page can tell
    /// "you may not" from "no such thing".
    PermissionDenied,
    MethodNotFound,
    InvalidParams,
    /// The call was well-formed and permitted, but the operation failed (ENOENT, and friends).
    OperationFailed,
    /// Valid on another platform or session type — layer-shell under X11, for instance.
    Unsupported,
    /// The stream was cancelled by the page.
    Cancelled,
    Internal,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::PermissionDenied => "permissionDenied",
            ErrorCode::MethodNotFound => "methodNotFound",
            ErrorCode::InvalidParams => "invalidParams",
            ErrorCode::OperationFailed => "operationFailed",
            ErrorCode::Unsupported => "unsupported",
            ErrorCode::Cancelled => "cancelled",
            ErrorCode::Internal => "internal",
        }
    }
}

/// Split a `module.method` name, which is how every entry in the §9.3 catalog is addressed.
pub fn split_method(method: &str) -> Option<(&str, &str)> {
    method.split_once('.')
}
