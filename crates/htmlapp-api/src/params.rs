//! Typed parameter decoding for bridge calls.

use htmlapp_bridge::RpcError;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// Decode a call's params into a typed struct, reporting the failure to the page in a form its
/// author can act on rather than as an opaque internal error.
pub fn decode<T: DeserializeOwned>(method: &str, params: Value) -> Result<T, RpcError> {
    // A method whose params type is a unit struct still needs to accept `null`.
    let params = if params.is_null() {
        Value::Object(Default::default())
    } else {
        params
    };
    serde_json::from_value(params).map_err(|e| RpcError::invalid_params(format!("{method}: {e}")))
}
