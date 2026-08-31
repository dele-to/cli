use crate::api::{CreateShareRequest, OpaqueClient, DEFAULT_API_URL};
use crate::crypto::{decrypt_share, encrypt_share};
use crate::share_url::{build_share_url, parse_share_url, ShareLink};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ShareError {
    #[error(transparent)]
    Crypto(#[from] crate::crypto::CryptoError),
    #[error(transparent)]
    Api(#[from] crate::api::ApiError),
    #[error(transparent)]
    Url(#[from] crate::share_url::ShareUrlError),
}

#[derive(Debug, Clone)]
pub struct CreateOptions {
    pub expires_in: u64,
    pub max_views: u32,
}

#[derive(Debug, Clone)]
pub struct CreatedShare {
    pub id: String,
    pub share_url: String,
    pub expires_at: String,
    pub delete_capability: String,
}

#[derive(Debug, Clone)]
pub struct ViewedShare {
    pub id: String,
    pub plaintext: String,
    pub expires_at: String,
    pub remaining_views: u32,
}

pub fn create_share(
    client: &OpaqueClient,
    plaintext: &str,
    options: CreateOptions,
) -> Result<CreatedShare, ShareError> {
    let encrypted = encrypt_share(plaintext)?;
    let created = client.create_share(&CreateShareRequest {
        payload: encrypted.payload,
        expires_in: options.expires_in,
        max_views: options.max_views,
    })?;
    let base = created
        .share_url
        .clone()
        .unwrap_or_else(|| format!("https://dele.to/view/{}", created.id));
    Ok(CreatedShare {
        share_url: build_share_url(&base, &encrypted.root_secret.0, &created.read_capability)?,
        id: created.id,
        expires_at: created.expires_at,
        delete_capability: created.delete_capability,
    })
}

pub fn view_share(url: &str, api_url_override: Option<&str>) -> Result<ViewedShare, ShareError> {
    let mut link = parse_share_url(url)?;
    link.api_url = resolve_view_api_url(&link.api_url, api_url_override);
    view_parsed(&link)
}

fn resolve_view_api_url(origin: &str, api_url_override: Option<&str>) -> String {
    match api_url_override {
        Some(url) => {
            let trimmed = url.trim_end_matches('/');
            // A default production origin must not replace localhost (or any
            // other host) already present in the share URL.
            if trimmed == DEFAULT_API_URL {
                origin.trim_end_matches('/').to_string()
            } else {
                trimmed.to_string()
            }
        }
        None => origin.trim_end_matches('/').to_string(),
    }
}

pub fn view_parsed(link: &ShareLink) -> Result<ViewedShare, ShareError> {
    let client = OpaqueClient::new(&link.api_url, None)?;
    let viewed = client.view_share(&link.id, &link.fragment.read_capability)?;
    Ok(ViewedShare {
        id: link.id.clone(),
        plaintext: decrypt_share(&viewed.payload, &link.fragment)?,
        expires_at: viewed.expires_at,
        remaining_views: viewed.remaining_views,
    })
}

pub fn delete_share(
    api_url: &str,
    id: &str,
    delete_capability: &str,
) -> Result<(), ShareError> {
    OpaqueClient::new(api_url, None)?.delete_share(id, delete_capability)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_api_url_does_not_override_localhost_origin() {
        assert_eq!(
            resolve_view_api_url("http://localhost:3000", Some(DEFAULT_API_URL)),
            "http://localhost:3000"
        );
    }

    #[test]
    fn explicit_api_url_overrides_share_origin() {
        assert_eq!(
            resolve_view_api_url("https://dele.to", Some("http://localhost:3000")),
            "http://localhost:3000"
        );
    }
}
