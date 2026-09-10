//! `http` — a fetch that ignores CORS (docs/api-reference.md, Tier 1).
//!
//! CORS is a browser policy for protecting *other people's* origins from a page. Here the host is
//! The one deciding what may be reached, and it decides with the manifest's allow-list, which is
//! both stricter and more legible than CORS would be. The threat model names exfiltration as the threat this
//! answers, so the check happens on every request and on every redirect hop.

use htmlapp_bridge::RpcError;
use htmlapp_bridge::dispatch::{ApiHandler, BoxFuture, ValueStream};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;

use crate::context::Ctx;
use crate::params::decode;

pub struct HttpModule {
    ctx: Ctx,
    #[cfg(feature = "tier1")]
    client: reqwest::Client,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RequestParams {
    url: String,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

impl HttpModule {
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            #[cfg(feature = "tier1")]
            client: reqwest::Client::builder()
                // Redirects are followed manually so each hop can be re-checked against the
                // allow-list; otherwise a permitted origin could bounce the request anywhere.
                .redirect(reqwest::redirect::Policy::none())
                .user_agent(concat!("htmlapp/", env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap_or_default(),
        }
    }
}

#[cfg(feature = "tier1")]
impl HttpModule {
    /// Build and send a request, re-checking the allow-list at every redirect.
    async fn send(&self, params: &RequestParams) -> Result<reqwest::Response, RpcError> {
        const MAX_REDIRECTS: usize = 10;

        let mut url = params.url.clone();
        for _ in 0..MAX_REDIRECTS {
            self.ctx.check_origin(&url)?;

            let method: reqwest::Method = params
                .method
                .as_deref()
                .unwrap_or("GET")
                .parse()
                .map_err(|_| RpcError::invalid_params("unknown HTTP method"))?;

            let mut request = self.client.request(method, &url);
            for (name, value) in &params.headers {
                request = request.header(name, value);
            }
            if let Some(body) = &params.body {
                request = request.body(body.clone());
            }
            if let Some(ms) = params.timeout_ms {
                request = request.timeout(std::time::Duration::from_millis(ms));
            }

            let response = request.send().await.map_err(|e| {
                RpcError::new(htmlapp_bridge::ErrorCode::OperationFailed, e.to_string())
            })?;

            if response.status().is_redirection()
                && let Some(location) = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
            {
                // Resolve relative redirects against the URL that produced them.
                url = match reqwest::Url::parse(&url).and_then(|base| base.join(location)) {
                    Ok(next) => next.to_string(),
                    Err(_) => location.to_string(),
                };
                continue;
            }

            return Ok(response);
        }

        Err(RpcError::new(
            htmlapp_bridge::ErrorCode::OperationFailed,
            "too many redirects",
        ))
    }

    async fn fetch(&self, params: Value) -> Result<Value, RpcError> {
        let params: RequestParams = decode("http.fetch", params)?;
        let response = self.send(&params).await?;

        let status = response.status();
        let url = response.url().to_string();
        let headers: HashMap<String, String> = response
            .headers()
            .iter()
            .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.to_string(), v.to_string())))
            .collect();
        let body = response.text().await.unwrap_or_default();

        Ok(json!({
            "status": status.as_u16(),
            "statusText": status.canonical_reason().unwrap_or(""),
            "headers": headers,
            "body": body,
            "url": url,
        }))
    }
}

impl ApiHandler for HttpModule {
    fn name(&self) -> &'static str {
        "http"
    }

    fn invoke<'a>(&'a self, method: &'a str, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            match method {
                #[cfg(feature = "tier1")]
                "fetch" => self.fetch(params).await,
                #[cfg(not(feature = "tier1"))]
                "fetch" => {
                    let _ = params;
                    Err(RpcError::unsupported("this build has no HTTP client"))
                }
                other => Err(RpcError::not_found(&format!("http.{other}"))),
            }
        })
    }

    #[cfg(feature = "tier1")]
    fn open_stream<'a>(
        &'a self,
        method: &'a str,
        params: Value,
    ) -> BoxFuture<'a, Result<ValueStream, RpcError>> {
        Box::pin(async move {
            if method != "stream" {
                return Err(RpcError::not_found(&format!("http.{method}")));
            }
            let params: RequestParams = decode("http.stream", params)?;
            let response = self.send(&params).await?;

            let stream = async_stream::try_stream! {
                use futures::StreamExt as _;
                let mut bytes = response.bytes_stream();
                while let Some(chunk) = bytes.next().await {
                    let chunk = chunk.map_err(|e| RpcError::new(
                        htmlapp_bridge::ErrorCode::OperationFailed, e.to_string()))?;
                    yield json!(String::from_utf8_lossy(&chunk));
                }
            };
            Ok(Box::pin(stream) as ValueStream)
        })
    }
}
