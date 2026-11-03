//! Prometheus series and label-value lookups.
//!
//! A second Prometheus query path used by the topology view. It reads
//! `/api/v1/series` (GET for short selector lists, form-encoded POST for long
//! ones) and `/api/v1/label/<name>/values` from the same Prometheus server the
//! `prometheus` adapter talks to.

use crate::adapter::{self, BaseAdapter};
use crate::resilience::{IntegrationError, IntegrationResult};
use crate::{CircuitBreakerConfig, RateLimiterConfig, RetryConfig};
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::header::{ACCEPT, CONTENT_TYPE, USER_AGENT};
use hyper::{Method, Request, StatusCode};
use hyper_util::client::legacy::{connect::HttpConnector, Client};
use hyper_util::rt::TokioExecutor;
use serde::Deserialize;
use std::collections::BTreeMap;

/// Selector lists longer than this are sent as a form-encoded POST body.
const MAX_GET_MATCHERS: usize = 3;

#[derive(Debug, Deserialize)]
struct SeriesEnvelope {
    status: String,
    #[serde(default)]
    data: Vec<BTreeMap<String, String>>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ValuesEnvelope {
    status: String,
    #[serde(default)]
    data: Vec<String>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ErrorEnvelope {
    error: Option<String>,
}

/// Client for the Prometheus series and label-values endpoints.
pub struct PrometheusSeriesClient {
    base: BaseAdapter,
    client: Client<HttpConnector, Full<Bytes>>,
    base_url: String,
}

impl PrometheusSeriesClient {
    /// Create a client for the Prometheus server at `base_url`.
    pub fn new(
        id: impl Into<String>,
        base_url: impl Into<String>,
        circuit_breaker_config: CircuitBreakerConfig,
        rate_limiter_config: RateLimiterConfig,
        retry_config: RetryConfig,
    ) -> Self {
        Self {
            base: BaseAdapter::new(
                id,
                adapter::IntegrationKind::TelemetryCollector,
                circuit_breaker_config,
                rate_limiter_config,
                retry_config,
            ),
            client: Client::builder(TokioExecutor::new()).build_http(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
        }
    }

    /// Series matching every selector in `matchers` between `start` and `end`
    /// (Unix seconds), each series as its sorted label set.
    pub async fn series(
        &self,
        matchers: &[&str],
        start: i64,
        end: i64,
    ) -> IntegrationResult<Vec<BTreeMap<String, String>>> {
        let mut form = format!("start={}&end={}", start, end);
        for matcher in matchers {
            form.push_str("&match[]=");
            form.push_str(&urlencoding::encode(matcher));
        }
        let use_post = matchers.len() > MAX_GET_MATCHERS;
        let url = if use_post {
            format!("{}/api/v1/series", self.base_url)
        } else {
            format!("{}/api/v1/series?{}", self.base_url, form)
        };
        let base = self.base.clone();
        let client = self.client.clone();

        base.execute_with_resilience(move || {
            let client = client.clone();
            let url = url.clone();
            let form = form.clone();
            async move {
                let builder = Request::builder()
                    .uri(&url)
                    .header(ACCEPT, "application/json")
                    .header(USER_AGENT, "rustops-integration/1.0");
                let request = if use_post {
                    builder
                        .method(Method::POST)
                        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                        .body(Full::new(Bytes::from(form)))?
                } else {
                    builder.method(Method::GET).body(Full::new(Bytes::new()))?
                };

                let response = client.request(request).await?;
                let status = response.status();
                let body = response.into_body().collect().await?.to_bytes();
                check_status(status, &body)?;

                let envelope: SeriesEnvelope = serde_json::from_slice(&body)?;
                if envelope.status != "success" {
                    return Err(IntegrationError::Unknown(
                        envelope
                            .error
                            .unwrap_or_else(|| "series query failed".to_string()),
                    ));
                }
                Ok(envelope.data)
            }
        })
        .await
    }

    /// Values of the label `name`, in the order Prometheus returns them.
    pub async fn label_values(&self, name: &str) -> IntegrationResult<Vec<String>> {
        let url = format!(
            "{}/api/v1/label/{}/values",
            self.base_url,
            urlencoding::encode(name)
        );
        let base = self.base.clone();
        let client = self.client.clone();

        base.execute_with_resilience(move || {
            let client = client.clone();
            let url = url.clone();
            async move {
                let request = Request::builder()
                    .method(Method::GET)
                    .uri(&url)
                    .header(ACCEPT, "application/json")
                    .header(USER_AGENT, "rustops-integration/1.0")
                    .body(Full::new(Bytes::new()))?;

                let response = client.request(request).await?;
                let status = response.status();
                let body = response.into_body().collect().await?.to_bytes();
                check_status(status, &body)?;

                let envelope: ValuesEnvelope = serde_json::from_slice(&body)?;
                if envelope.status != "success" {
                    return Err(IntegrationError::Unknown(
                        envelope
                            .error
                            .unwrap_or_else(|| "label values query failed".to_string()),
                    ));
                }
                Ok(envelope.data)
            }
        })
        .await
    }
}

/// Client errors carry the Prometheus error text; server errors are network faults.
fn check_status(status: StatusCode, body: &[u8]) -> IntegrationResult<()> {
    if status.is_success() {
        return Ok(());
    }
    if status.is_client_error() {
        let detail = serde_json::from_slice::<ErrorEnvelope>(body)
            .ok()
            .and_then(|e| e.error)
            .unwrap_or_else(|| status.to_string());
        return Err(IntegrationError::InvalidResponse(detail));
    }
    Err(IntegrationError::Network(format!("HTTP {}", status)))
}
