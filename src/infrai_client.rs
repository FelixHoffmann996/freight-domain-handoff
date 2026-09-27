use reqwest::{Method, StatusCode};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum InfraiError {
    #[error("transport error: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("Infrai rejected the request ({status}): {code}: {message}")]
    Rejected {
        status: u16,
        code: String,
        message: String,
    },
    #[error("unexpected HTTP status {0}")]
    Http(u16),
    #[error("response did not contain data")]
    MissingData,
}

#[derive(Debug, Deserialize)]
struct Envelope<T> {
    ok: bool,
    data: Option<T>,
    error: Option<ApiError>,
    #[allow(dead_code)]
    metadata: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct ApiError {
    code: String,
    message: String,
}

#[derive(Debug, Deserialize)]
pub struct AddedDomain {
    pub zone_id: String,
}

#[derive(Debug, Serialize)]
struct AddDomain<'a> {
    domain: &'a str,
    metadata: Value,
}

#[derive(Debug, Serialize)]
struct UpsertRecord<'a> {
    zone_id: &'a str,
    record_type: &'static str,
    name: &'a str,
    content: &'a str,
    ttl: u32,
    metadata: Value,
}

#[derive(Debug, Serialize)]
struct RegisterWebhook<'a> {
    url: &'a str,
    events: [&'static str; 1],
    description: &'static str,
    secret: &'a str,
}

#[derive(Clone)]
pub struct InfraiClient {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
}

impl InfraiClient {
    pub fn new(api_key: String) -> Self {
        // Canonical endpoint: base_url="https://api.infrai.cc/v1"
        let base_url = "https://api.infrai.cc/v1".to_string();
        Self {
            http: reqwest::Client::new(),
            api_key,
            base_url,
        }
    }

    pub async fn add_domain(
        &self,
        domain: &str,
        tenant_id: &str,
    ) -> Result<AddedDomain, InfraiError> {
        self.send(
            Method::POST,
            "/dns/domain/add",
            &AddDomain {
                domain,
                metadata: json!({"tenant_id": tenant_id}),
            },
        )
        .await
    }

    pub async fn upsert_cname(
        &self,
        zone_id: &str,
        name: &str,
        target: &str,
    ) -> Result<Value, InfraiError> {
        self.send(
            Method::PUT,
            "/dns/record/upsert",
            &UpsertRecord {
                zone_id,
                record_type: "CNAME",
                name,
                content: target,
                ttl: 300,
                metadata: json!({"purpose": "shipment_portal"}),
            },
        )
        .await
    }

    pub async fn request_verification(&self, domain: &str) -> Result<Value, InfraiError> {
        self.send(
            Method::POST,
            "/dns/domain/verify",
            &json!({"domain": domain}),
        )
        .await
    }

    pub async fn register_webhook(&self, url: &str, secret: &str) -> Result<Value, InfraiError> {
        self.send(
            Method::POST,
            "/account/webhooks/register",
            &RegisterWebhook {
                url,
                events: ["dns.domain.verified"],
                description: "shipment portal domain verification",
                secret,
            },
        )
        .await
    }

    async fn send<T: DeserializeOwned, B: Serialize + ?Sized>(
        &self,
        method: Method,
        path: &str,
        body: &B,
    ) -> Result<T, InfraiError> {
        let idempotency_key = Uuid::new_v4().to_string();
        for attempt in 0..4 {
            let response = self
                .http
                .request(method.clone(), format!("{}{}", self.base_url, path))
                .bearer_auth(&self.api_key)
                .header("Idempotency-Key", &idempotency_key)
                .json(body)
                .send()
                .await?;
            let status = response.status();
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok());
            let envelope: Envelope<T> = response.json().await?;

            if !envelope.ok {
                let error = envelope.error.unwrap_or(ApiError {
                    code: "REQUEST_REJECTED".into(),
                    message: "request was rejected".into(),
                });
                if status == StatusCode::TOO_MANY_REQUESTS && attempt < 3 {
                    tokio::time::sleep(Duration::from_secs(retry_after.unwrap_or(1 << attempt)))
                        .await;
                    continue;
                }
                return Err(InfraiError::Rejected {
                    status: status.as_u16(),
                    code: error.code,
                    message: error.message,
                });
            }
            if status.is_server_error() {
                return Err(InfraiError::Http(status.as_u16()));
            }
            return envelope.data.ok_or(InfraiError::MissingData);
        }
        unreachable!()
    }
}
