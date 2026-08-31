use thiserror::Error;
use url::Url;

use crate::crypto::{decode_share_fragment, encode_share_fragment, SecretType, ShareFragment};

#[derive(Debug, Error)]
pub enum ShareUrlError {
    #[error("not a Deleto share URL")]
    InvalidUrl,
    #[error("share URL is missing the secret fragment")]
    MissingFragment,
    #[error(transparent)]
    Crypto(#[from] crate::crypto::CryptoError),
}

#[derive(Debug, Clone)]
pub struct ShareLink {
    pub api_url: String,
    pub id: String,
    pub fragment: ShareFragment,
}

pub fn parse_share_url(value: &str) -> Result<ShareLink, ShareUrlError> {
    let parsed = Url::parse(value).map_err(|_| ShareUrlError::InvalidUrl)?;
    let id = parsed
        .path_segments()
        .and_then(|mut segments| {
            let first = segments.next()?;
            let second = segments.next();
            match (first, second) {
                ("view", Some(id)) => Some(id.to_string()),
                _ => None,
            }
        })
        .ok_or(ShareUrlError::InvalidUrl)?;
    if id.is_empty() {
        return Err(ShareUrlError::InvalidUrl);
    }
    let fragment = parsed.fragment().ok_or(ShareUrlError::MissingFragment)?;
    Ok(ShareLink {
        api_url: origin(&parsed),
        id,
        fragment: decode_share_fragment(fragment)?,
    })
}

pub fn build_share_url(
    share_url: &str,
    root_secret: &[u8; 32],
    read_capability: &str,
) -> Result<String, ShareUrlError> {
    let fragment = encode_share_fragment(root_secret, read_capability, SecretType::Root)?;
    let mut parsed = Url::parse(share_url).map_err(|_| ShareUrlError::InvalidUrl)?;
    parsed.set_fragment(Some(&fragment));
    Ok(parsed.to_string())
}

fn origin(url: &Url) -> String {
    let host = url.host_str().unwrap_or("dele.to");
    match url.port() {
        Some(port) => format!("{}://{host}:{port}", url.scheme()),
        None => format!("{}://{host}", url.scheme()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{encode_base64url, encode_share_fragment, READ_CAPABILITY_PREFIX};

    #[test]
    fn roundtrips_a_v1_share_link() {
        let secret = [11u8; 32];
        let capability = format!("{READ_CAPABILITY_PREFIX}{}", encode_base64url(&[5u8; 32]));
        let fragment = encode_share_fragment(&secret, &capability, SecretType::Root).unwrap();
        let url = format!("https://dele.to/view/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee#{fragment}");
        let link = parse_share_url(&url).unwrap();
        assert_eq!(link.api_url, "https://dele.to");
        assert_eq!(link.id, "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee");
        assert_eq!(link.fragment.read_capability, capability);
        assert_eq!(link.fragment.secret, secret);
    }

    #[test]
    fn keeps_localhost_origin_for_the_api() {
        let secret = [1u8; 32];
        let capability = format!("{READ_CAPABILITY_PREFIX}{}", encode_base64url(&[2u8; 32]));
        let fragment = encode_share_fragment(&secret, &capability, SecretType::Root).unwrap();
        let url = format!("http://localhost:3000/view/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee#{fragment}");
        let link = parse_share_url(&url).unwrap();
        assert_eq!(link.api_url, "http://localhost:3000");
    }

    #[test]
    fn parses_a_website_root_secret_fragment_on_localhost() {
        let fragment = "0QAMVXtXv4QhY4lq578qWzzwFX5-WICsjg9ydlp2cyM";
        let url = format!("http://localhost:3000/view/de48ba92-88a6-4e80-bed5-e0b680719d03#{fragment}");
        let link = parse_share_url(&url).unwrap();
        assert_eq!(link.api_url, "http://localhost:3000");
        assert_eq!(link.id, "de48ba92-88a6-4e80-bed5-e0b680719d03");
        assert_eq!(link.fragment.secret_type, SecretType::Root);
        assert_eq!(encode_base64url(&link.fragment.secret), fragment);
    }
}
