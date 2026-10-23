//! Mock HTTP server for testing.

use std::collections::HashMap;
use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::http::StatusCode;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Mock HTTP response.
#[derive(Debug, Clone)]
pub struct MockResponse {
    pub status: StatusCode,
    pub body: String,
    pub headers: HashMap<String, String>,
}

impl MockResponse {
    /// Create a new mock response.
    pub fn new(status: StatusCode, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
            headers: HashMap::new(),
        }
    }

    /// Create a 200 OK response.
    pub fn ok(body: impl Into<String>) -> Self {
        Self::new(StatusCode::OK, body)
    }

    /// Create a 404 Not Found response.
    pub fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "Not Found")
    }

    /// Create a 500 Internal Server Error response.
    pub fn internal_error() -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error")
    }

    /// Add a header.
    pub fn header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }
}

/// Mock HTTP server for testing.
pub struct MockHttpServer {
    responses: Arc<RwLock<HashMap<String, MockResponse>>>,
    port: u16,
}

impl MockHttpServer {
    /// Create a new mock HTTP server.
    pub fn new() -> Self {
        Self {
            responses: Arc::new(RwLock::new(HashMap::new())),
            port: 0,
        }
    }

    /// Set a response for a path.
    pub async fn set_response(&self, path: impl Into<String>, response: MockResponse) {
        let mut responses = self.responses.write().await;
        responses.insert(path.into(), response);
    }

    /// Get the server URL.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Start the server.
    pub async fn start(&mut self) -> std::io::Result<()> {
        let responses = self.responses.clone();

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
        self.port = listener.local_addr()?.port();

        let _ = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let responses = responses.clone();
                tokio::spawn(async move {
                    let service = service_fn(move |req| {
                        let responses = responses.clone();
                        async move { Self::handle_request(req, responses).await }
                    });
                    let _ = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });

        // Give server time to start
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        Ok(())
    }

    async fn handle_request(
        req: Request<Incoming>,
        responses: Arc<RwLock<HashMap<String, MockResponse>>>,
    ) -> Result<Response<Full<Bytes>>, Infallible> {
        let path = req.uri().path().to_string();

        let responses = responses.read().await;
        let mock_response = responses.get(&path)
            .cloned()
            .unwrap_or_else(|| MockResponse::not_found());

        let mut builder = Response::builder()
            .status(mock_response.status);

        for (key, value) in mock_response.headers {
            builder = builder.header(key, value);
        }

        // A status or header the test configured that http rejects is answered as a 500 naming the problem.
        let response = builder
            .body(Full::new(Bytes::from(mock_response.body)))
            .unwrap_or_else(|error| {
                let mut response = Response::new(Full::new(Bytes::from(error.to_string())));
                *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
                response
            });
        Ok(response)
    }
}

impl Default for MockHttpServer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_http_server() {
        let mut server = MockHttpServer::new();
        server.start().await.unwrap();

        server.set_response(
            "/test",
            MockResponse::ok("Hello, World!")
        ).await;

        let client = reqwest::Client::new();
        let response = client.get(&format!("{}/test", server.url()))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
        assert_eq!(response.text().await.unwrap(), "Hello, World!");
    }

    #[tokio::test]
    async fn test_mock_response_builder() {
        let response = MockResponse::ok("test")
            .header("Content-Type", "text/plain")
            .header("X-Custom", "value");

        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.body, "test");
        assert_eq!(response.headers.len(), 2);
    }
}
