use axum::body::HttpBody;
use axum::extract::Request;
use axum::http::{header, HeaderValue, Method};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::{http::StatusCode, response::Json, routing::get, Router};
use serde::Serialize;
use std::net::SocketAddr;
use tokio::signal;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

/// Health check response
#[derive(Debug, Clone, Serialize)]
struct HealthResponse {
    status: String,
    version: String,
}

/// Metrics response placeholder
#[derive(Debug, Clone, Serialize)]
struct MetricsResponse {
    metrics: String,
}

/// Health check handler
async fn health_check() -> Result<Json<HealthResponse>, ApiError> {
    let response = HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    };
    Ok(Json(response))
}

/// Metrics handler (placeholder for future telemetry integration)
async fn metrics() -> Result<Json<MetricsResponse>, ApiError> {
    let response = MetricsResponse {
        metrics: "Metrics will be provided by rustops-telemetry crate".to_string(),
    };
    Ok(Json(response))
}

/// API error type
#[derive(Debug)]
enum ApiError {
    /// Wired into `IntoResponse` for future handlers; the current
    /// placeholder endpoints never fail, so nothing constructs it yet.
    #[allow(dead_code)]
    Internal(String),
}

impl axum::response::IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match self {
            ApiError::Internal(msg) => {
                error!("Internal API error: {}", msg);
                (StatusCode::INTERNAL_SERVER_ERROR, msg)
            }
        };
        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}

/// Create the API router
fn create_router() -> Router {
    Router::new()
        .route("/health", get(health_check))
        .route("/metrics", get(metrics))
        .nest("/api/v1", v1_router())
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .layer(middleware::from_fn(legacy_wire_shape))
}

/// Keep the response head the axum 0.7 / tower-http 0.5 stack put on the wire.
///
/// - tower-http 0.5 emitted `Access-Control-Allow-Origin` first on CORS preflight answers; 0.6 emits it
///   after `Vary` and the allow-methods/allow-headers pair.
/// - axum 0.7 answered a HEAD request whose response has an empty body with `Content-Length: 0`; axum 0.8
///   sends no length for it (the unrouted fallback, for instance).
async fn legacy_wire_shape(request: Request, next: Next) -> Response {
    let head = request.method() == Method::HEAD;
    let mut response = next.run(request).await;
    let empty = response.body().size_hint().exact() == Some(0);
    let headers = response.headers_mut();
    if headers.contains_key(header::ACCESS_CONTROL_ALLOW_METHODS) {
        if let Some(origin) = headers.remove(header::ACCESS_CONTROL_ALLOW_ORIGIN) {
            let rest = std::mem::take(headers);
            headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
            headers.extend(rest);
        }
    }
    if head
        && empty
        && !headers.contains_key(header::CONTENT_LENGTH)
        && !headers.contains_key(header::TRANSFER_ENCODING)
    {
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from_static("0"));
    }
    response
}

/// API v1 router (placeholder for future endpoints)
fn v1_router() -> Router {
    Router::new().route("/", get(|| async { "RustOps API v1" }))
    // Future endpoints will be added here:
    // .route("/incidents", get(incidents_list))
    // .route("/incidents/:id", get(incident_detail))
    // .route("/anomalies", get(anomalies_list))
    // .route("/topology", get(topology_status))
}

/// Graceful shutdown handler
async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            info!("Received Ctrl+C, shutting down gracefully...");
        }
        _ = terminate => {
            info!("Received terminate signal, shutting down gracefully...");
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "rustops_api=debug,tower_http=debug,axum=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    info!("RustOps API server starting on {}", addr);

    let app = create_router();

    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("Server listening on {}", addr);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    info!("Server shutdown complete");
    Ok(())
}
