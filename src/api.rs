use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

pub const OPAQUE_MEDIA_TYPE: &str = "application/vnd.deleto.opaque+json;v=1";
pub const DEFAULT_API_URL: &str = "https://dele.to";

pub fn api_url_from_env() -> String {
    std::env::var("DELETO_API_URL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_API_URL.to_string())
}

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("{message}")]
    Problem {
        status: u16,
        code: String,
        message: String,
    },
    #[error("unexpected API response ({status}) from {url}")]
    Unexpected { status: u16, url: String, body: String },
    #[error(transparent)]
    Transport(#[from] reqwest::Error),
}

#[derive(Debug, Clone)]
pub struct OpaqueClient {
    http: reqwest::blocking::Client,
    api_url: String,
    api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CreateShareRequest {
    pub payload: String,
    pub expires_in: u64,
    pub max_views: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateShareResponse {
    pub id: String,
    pub expires_at: String,
    pub read_capability: String,
    pub delete_capability: String,
    pub share_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ViewShareResponse {
    pub payload: String,
    pub expires_at: String,
    pub remaining_views: u32,
}

#[derive(Debug, Deserialize)]
struct ProblemBody {
    title: Option<String>,
    code: Option<String>,
}

impl OpaqueClient {
    pub fn new(api_url: impl Into<String>, api_key: Option<String>) -> Result<Self, ApiError> {
        let http = reqwest::blocking::Client::builder()
            .user_agent(concat!("deleto-cli/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            http,
            api_url: api_url.into().trim_end_matches('/').to_string(),
            api_key,
        })
    }

    pub fn create_share(&self, request: &CreateShareRequest) -> Result<CreateShareResponse, ApiError> {
        let body = serde_json::to_vec(request).map_err(|_| ApiError::Problem {
            status: 400,
            code: "invalid_request".into(),
            message: "Failed to encode the opaque share request".into(),
        })?;
        let mut builder = self
            .http
            .post(format!("{}/api/v1/shares", self.api_url))
            .header("Content-Type", OPAQUE_MEDIA_TYPE)
            .header("Idempotency-Key", Uuid::new_v4().to_string())
            .body(body);
        if let Some(key) = &self.api_key {
            builder = builder.bearer_auth(key);
        }
        parse_json(builder.send()?)
    }

    pub fn view_share(&self, id: &str, read_capability: &str) -> Result<ViewShareResponse, ApiError> {
        if read_capability.is_empty() {
            return Err(ApiError::Problem {
                status: 400,
                code: "invalid_capability".into(),
                message: "Share URL is missing a read capability fragment".into(),
            });
        }
        let response = self
            .http
            .post(format!("{}/api/v1/shares/{id}/views", self.api_url))
            .header("Idempotency-Key", Uuid::new_v4().to_string())
            .bearer_auth(read_capability)
            .send()?;
        parse_json(response)
    }

    pub fn delete_share(&self, id: &str, delete_capability: &str) -> Result<(), ApiError> {
        let response = self
            .http
            .delete(format!("{}/api/v1/shares/{id}", self.api_url))
            .bearer_auth(delete_capability)
            .send()?;
        if response.status().as_u16() == 204 {
            return Ok(());
        }
        parse_json::<serde_json::Value>(response).map(|_| ())
    }
}

fn parse_json<T: serde::de::DeserializeOwned>(response: reqwest::blocking::Response) -> Result<T, ApiError> {
    let url = response.url().to_string();
    let status = response.status().as_u16();
    let body = response.text()?;
    if (200..300).contains(&status) {
        return serde_json::from_str(&body).map_err(|_| ApiError::Unexpected {
            status,
            url,
            body,
        });
    }
    if let Ok(problem) = serde_json::from_str::<ProblemBody>(&body) {
        return Err(ApiError::Problem {
            status,
            code: problem.code.unwrap_or_else(|| "api_error".into()),
            message: problem.title.unwrap_or_else(|| format!("API error ({status})")),
        });
    }
    Err(ApiError::Unexpected { status, url, body })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_request_only_has_opaque_fields() {
        let request = CreateShareRequest {
            payload: "aaaaaaaaaaaaaaaaaaaaaaaa".into(),
            expires_in: 3600,
            max_views: 1,
        };
        let value = serde_json::to_value(&request).unwrap();
        let mut keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["expires_in", "max_views", "payload"]);
        assert!(!serde_json::to_string(&request).unwrap().contains("plaintext"));
    }
}
