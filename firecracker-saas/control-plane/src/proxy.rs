use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use hyper_util::{client::legacy::Client, rt::TokioExecutor};

use crate::AppState;

/// Middleware: if `x-tenant-id` header is present, proxy the request to the tenant's VM.
/// Otherwise, pass through to normal API routes.
pub async fn proxy_middleware(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response {
    let tenant_id = match req.headers().get("x-tenant-id") {
        Some(v) => match v.to_str() {
            Ok(s) => s.to_string(),
            Err(_) => return (StatusCode::BAD_REQUEST, "invalid x-tenant-id").into_response(),
        },
        None => return next.run(req).await,
    };

    let vm_ip = {
        let manager = state.tenant_manager.read().await;
        match manager.get_tenant(&tenant_id) {
            Some(t) => t.vm_ip.clone(),
            None => return (StatusCode::NOT_FOUND, "tenant not found").into_response(),
        }
    };

    let query = req
        .uri()
        .query()
        .map(|q| format!("?{}", q))
        .unwrap_or_default();

    let upstream_uri: hyper::Uri = format!("http://{}:8080{}{}", vm_ip, req.uri().path(), query)
        .parse()
        .unwrap();

    let client = Client::builder(TokioExecutor::new()).build_http();

    let (mut parts, body) = req.into_parts();
    parts.uri = upstream_uri;
    parts.headers.remove("x-tenant-id");
    parts
        .headers
        .insert("host", HeaderValue::from_str(&format!("{}:8080", vm_ip)).unwrap());

    let upstream_req = Request::from_parts(parts, body);

    match client.request(upstream_req).await {
        Ok(resp) => {
            let (parts, body) = resp.into_parts();
            Response::from_parts(parts, Body::new(body))
        }
        Err(e) => {
            tracing::error!("Proxy error for tenant '{}' ({}): {}", tenant_id, vm_ip, e);
            (StatusCode::BAD_GATEWAY, format!("proxy error: {}", e)).into_response()
        }
    }
}
