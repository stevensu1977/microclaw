use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    middleware,
    response::IntoResponse,
    routing::{delete, get, post, put},
    Json, Router,
};
use serde::Deserialize;
use tower_http::trace::TraceLayer;

use crate::tenant::{CreateTenantRequest, Tier};
use crate::AppState;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        // 租户 CRUD
        .route("/api/v1/tenants", post(create_tenant))
        .route("/api/v1/tenants", get(list_tenants))
        .route("/api/v1/tenants/:id", get(get_tenant))
        .route("/api/v1/tenants/:id", delete(delete_tenant))
        // 生命周期
        .route("/api/v1/tenants/:id/start", post(start_tenant))
        .route("/api/v1/tenants/:id/stop", post(stop_tenant))
        .route("/api/v1/tenants/:id/pause", post(pause_tenant))
        .route("/api/v1/tenants/:id/resume", post(resume_tenant))
        .route("/api/v1/tenants/:id/snapshot", post(snapshot_tenant))
        // 配置
        .route("/api/v1/tenants/:id/env", put(update_tenant_env))
        // 健康检查
        .route("/api/v1/tenants/:id/health", get(tenant_health))
        .route("/health", get(health))
        // Debug: register a mock tenant (for testing without Firecracker)
        .route("/api/v1/debug/register_tenant", post(debug_register_tenant))
        // Metrics
        .route("/metrics", get(metrics))
        .layer(middleware::from_fn_with_state(state.clone(), crate::proxy::proxy_middleware))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

#[derive(Deserialize)]
struct CreateTenantBody {
    tenant_id: String,
    tier: String,
    #[serde(default)]
    channels: Vec<String>,
    #[serde(default)]
    env_vars: std::collections::HashMap<String, String>,
}

async fn create_tenant(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateTenantBody>,
) -> impl IntoResponse {
    let tier = match body.tier.as_str() {
        "free" => Tier::Free,
        "pro" => Tier::Pro,
        "team" => Tier::Team,
        "enterprise" => Tier::Enterprise,
        _ => return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "invalid tier"}))),
    };

    let req = CreateTenantRequest {
        tenant_id: body.tenant_id,
        tier,
        channels: body.channels,
        env_vars: body.env_vars,
    };

    let mut manager = state.tenant_manager.write().await;
    match manager.create_tenant(req).await {
        Ok(tenant) => (StatusCode::CREATED, Json(serde_json::to_value(&tenant).unwrap())),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ),
    }
}

async fn list_tenants(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let manager = state.tenant_manager.read().await;
    let tenants = manager.list_tenants();
    Json(serde_json::to_value(&tenants).unwrap())
}

async fn get_tenant(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let manager = state.tenant_manager.read().await;
    match manager.get_tenant(&id) {
        Some(tenant) => (StatusCode::OK, Json(serde_json::to_value(&tenant).unwrap())),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "tenant not found"})),
        ),
    }
}

async fn delete_tenant(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let mut manager = state.tenant_manager.write().await;
    match manager.delete_tenant(&id).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"status": "deleted"}))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ),
    }
}

async fn start_tenant(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let mut manager = state.tenant_manager.write().await;
    match manager.start_tenant(&id).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"status": "started"}))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ),
    }
}

async fn stop_tenant(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let mut manager = state.tenant_manager.write().await;
    match manager.stop_tenant(&id).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"status": "stopped"}))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ),
    }
}

async fn pause_tenant(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let mut manager = state.tenant_manager.write().await;
    match manager.pause_tenant(&id).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"status": "paused"}))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ),
    }
}

async fn resume_tenant(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let mut manager = state.tenant_manager.write().await;
    match manager.resume_tenant(&id).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"status": "resumed"}))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ),
    }
}

async fn snapshot_tenant(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let mut manager = state.tenant_manager.write().await;
    match manager.snapshot_tenant(&id).await {
        Ok(path) => (StatusCode::OK, Json(serde_json::json!({"snapshot_path": path}))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ),
    }
}

#[derive(Deserialize)]
struct UpdateEnvBody {
    env_vars: std::collections::HashMap<String, String>,
}

async fn update_tenant_env(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateEnvBody>,
) -> impl IntoResponse {
    let mut manager = state.tenant_manager.write().await;
    match manager.update_env(&id, body.env_vars).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"status": "updated"}))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ),
    }
}

async fn tenant_health(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let manager = state.tenant_manager.read().await;
    match manager.check_health(&id).await {
        Ok(health) => (StatusCode::OK, Json(serde_json::to_value(&health).unwrap())),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ),
    }
}

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({"status": "ok"}))
}

async fn metrics(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let manager = state.tenant_manager.read().await;
    let tenants = manager.list_tenants();

    let total = tenants.len();
    let running = tenants.iter().filter(|t| t.status == crate::tenant::TenantStatus::Running).count();
    let stopped = tenants.iter().filter(|t| t.status == crate::tenant::TenantStatus::Stopped).count();
    let paused = tenants.iter().filter(|t| t.status == crate::tenant::TenantStatus::Paused).count();

    let body = format!(
        "# HELP microclaw_tenants_total Total number of tenants\n\
         # TYPE microclaw_tenants_total gauge\n\
         microclaw_tenants_total {total}\n\
         # HELP microclaw_tenants_by_status Tenants by status\n\
         # TYPE microclaw_tenants_by_status gauge\n\
         microclaw_tenants_by_status{{status=\"running\"}} {running}\n\
         microclaw_tenants_by_status{{status=\"stopped\"}} {stopped}\n\
         microclaw_tenants_by_status{{status=\"paused\"}} {paused}\n"
    );

    (StatusCode::OK, [("content-type", "text/plain; version=0.0.4")], body)
}

#[derive(Deserialize)]
struct DebugRegisterBody {
    tenant_id: String,
    vm_ip: String,
}

async fn debug_register_tenant(
    State(state): State<Arc<AppState>>,
    Json(body): Json<DebugRegisterBody>,
) -> impl IntoResponse {
    use crate::tenant::{Tenant, TenantStatus, Tier};

    let tenant = Tenant {
        id: body.tenant_id.clone(),
        tier: Tier::Pro,
        status: TenantStatus::Running,
        vm_ip: body.vm_ip,
        gateway_ip: String::new(),
        tap_device: String::new(),
        socket_path: String::new(),
        data_dir: String::new(),
        vm_pid: None,
        channels: vec!["web".into()],
        created_at: chrono::Utc::now(),
    };

    let mut manager = state.tenant_manager.write().await;
    manager.register_tenant(tenant);
    (StatusCode::CREATED, Json(serde_json::json!({"status": "registered"})))
}
