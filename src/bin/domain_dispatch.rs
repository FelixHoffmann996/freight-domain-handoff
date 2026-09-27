use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use freight_domain_handoff::{
    freight::Shipment,
    infrai_client::{InfraiClient, InfraiError},
};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::{collections::HashMap, env, sync::Arc};
use thiserror::Error;
use tokio::sync::RwLock;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
struct AppState {
    infrai: InfraiClient,
    webhook_secret: String,
    cname_target: String,
    domains: Arc<RwLock<HashMap<String, DomainState>>>,
    shipments: Arc<RwLock<HashMap<String, Shipment>>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
enum DomainState {
    Verifying,
    Ready,
}

#[derive(Debug, Deserialize)]
struct OnboardDomain {
    tenant_id: String,
    domain: String,
    callback_url: String,
}

#[derive(Debug, Serialize)]
struct OnboardResult {
    domain: String,
    zone_id: String,
    state: DomainState,
}

#[derive(Debug, Deserialize)]
struct VerificationNotice {
    event: String,
    domain: String,
}

#[derive(Debug, Deserialize)]
struct NewShipment {
    tracking_id: String,
}

#[derive(Debug, Error)]
enum ServiceError {
    #[error(transparent)]
    Infrai(#[from] InfraiError),
    #[error("invalid webhook signature")]
    BadSignature,
    #[error("invalid request: {0}")]
    BadRequest(String),
}

impl IntoResponse for ServiceError {
    fn into_response(self) -> Response {
        let status = match &self {
            Self::BadSignature | Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Infrai(InfraiError::Rejected { status, .. }) if *status < 500 => {
                StatusCode::from_u16(*status).unwrap_or(StatusCode::BAD_REQUEST)
            }
            Self::Infrai(_) => StatusCode::BAD_GATEWAY,
        };
        (status, Json(serde_json::json!({"error": self.to_string()}))).into_response()
    }
}

#[tokio::main]
async fn main() {
    let api_key = env::var("INFRAI_API_KEY").expect("INFRAI_API_KEY must be set");
    let webhook_secret =
        env::var("INFRAI_WEBHOOK_SECRET").expect("INFRAI_WEBHOOK_SECRET must be set");
    let cname_target = env::var("PORTAL_CNAME_TARGET").expect("PORTAL_CNAME_TARGET must be set");
    let state = AppState {
        infrai: InfraiClient::new(api_key),
        webhook_secret,
        cname_target,
        domains: Default::default(),
        shipments: Default::default(),
    };
    let app = Router::new()
        .route("/domains", post(onboard_domain))
        .route("/webhooks/infrai", post(domain_verified))
        .route("/shipments", post(create_shipment))
        .route("/health", get(|| async { "ok" }))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:8080")
        .await
        .unwrap();
    println!("domain-dispatch listening on http://127.0.0.1:8080");
    axum::serve(listener, app).await.unwrap();
}

async fn onboard_domain(
    State(state): State<AppState>,
    Json(input): Json<OnboardDomain>,
) -> Result<Json<OnboardResult>, ServiceError> {
    let added = state
        .infrai
        .add_domain(&input.domain, &input.tenant_id)
        .await?;
    state
        .infrai
        .upsert_cname(&added.zone_id, &input.domain, &state.cname_target)
        .await?;
    state
        .infrai
        .register_webhook(&input.callback_url, &state.webhook_secret)
        .await?;
    state.infrai.request_verification(&input.domain).await?;
    state
        .domains
        .write()
        .await
        .insert(input.domain.clone(), DomainState::Verifying);
    Ok(Json(OnboardResult {
        domain: input.domain,
        zone_id: added.zone_id,
        state: DomainState::Verifying,
    }))
}

async fn domain_verified(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, ServiceError> {
    let signature = headers
        .get("x-infrai-signature")
        .and_then(|v| v.to_str().ok())
        .ok_or(ServiceError::BadSignature)?;
    verify_signature(&state.webhook_secret, &body, signature)?;
    let notice: VerificationNotice =
        serde_json::from_slice(&body).map_err(|e| ServiceError::BadRequest(e.to_string()))?;
    if notice.event == "dns.domain.verified" {
        state
            .domains
            .write()
            .await
            .insert(notice.domain, DomainState::Ready);
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn create_shipment(
    State(state): State<AppState>,
    Json(input): Json<NewShipment>,
) -> Json<Shipment> {
    let shipment = Shipment::new(input.tracking_id.clone());
    state
        .shipments
        .write()
        .await
        .insert(input.tracking_id, shipment.clone());
    Json(shipment)
}

fn verify_signature(secret: &str, body: &[u8], supplied: &str) -> Result<(), ServiceError> {
    let bytes = hex::decode(supplied.strip_prefix("sha256=").unwrap_or(supplied))
        .map_err(|_| ServiceError::BadSignature)?;
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).map_err(|_| ServiceError::BadSignature)?;
    mac.update(body);
    mac.verify_slice(&bytes)
        .map_err(|_| ServiceError::BadSignature)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_webhook_signature_over_raw_body() {
        let body = br#"{"event":"dns.domain.verified","domain":"track.example.com"}"#;
        let mut mac = HmacSha256::new_from_slice(b"local-secret").unwrap();
        mac.update(body);
        let signature = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
        assert!(verify_signature("local-secret", body, &signature).is_ok());
    }
}
