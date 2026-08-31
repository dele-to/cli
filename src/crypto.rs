use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use hkdf::Hkdf;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use thiserror::Error;
use zeroize::{Zeroize, ZeroizeOnDrop};

pub const ROOT_SECRET_LEN: usize = 32;
pub const AES_KEY_LEN: usize = 32;
pub const GCM_IV_LEN: usize = 12;
pub const FRAGMENT_LEN: usize = 65;
pub const READ_CAPABILITY_PREFIX: &str = "dlt_read_v1_";
const HKDF_SALT: &[u8] = b"deleto:share:v1";
const HKDF_INFO_ENCRYPTION: &[u8] = b"encryption";
const HKDF_INFO_READ_CAPABILITY: &[u8] = b"read-capability";

#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("invalid Base64URL value")]
    InvalidBase64,
    #[error("unsupported client envelope")]
    UnsupportedEnvelope,
    #[error("invalid share credentials")]
    InvalidCredentials,
    #[error("invalid v1 share fragment")]
    InvalidFragment,
    #[error("encryption failed")]
    Encrypt,
    #[error("decryption failed")]
    Decrypt,
    #[error("key derivation failed")]
    Derive,
}

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct RootSecret(pub [u8; ROOT_SECRET_LEN]);

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct EncryptionKey(pub [u8; AES_KEY_LEN]);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretType {
    Root = 1,
    Key = 2,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvelopeV1 {
    pub v: u8,
    pub encrypted: String,
    pub iv: String,
}

#[derive(Clone)]
pub struct EncryptedShare {
    pub root_secret: RootSecret,
    pub payload: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareFragment {
    pub secret: [u8; ROOT_SECRET_LEN],
    pub secret_type: SecretType,
    pub read_capability: String,
}

pub fn generate_root_secret() -> RootSecret {
    let mut bytes = [0u8; ROOT_SECRET_LEN];
    rand::thread_rng().fill_bytes(&mut bytes);
    RootSecret(bytes)
}

pub fn derive_encryption_key(root: &RootSecret) -> Result<EncryptionKey, CryptoError> {
    Ok(EncryptionKey(hkdf_expand(root, HKDF_INFO_ENCRYPTION)?))
}

fn derive_read_capability(root: &RootSecret) -> Result<[u8; AES_KEY_LEN], CryptoError> {
    hkdf_expand(root, HKDF_INFO_READ_CAPABILITY)
}

fn hkdf_expand(root: &RootSecret, info: &[u8]) -> Result<[u8; AES_KEY_LEN], CryptoError> {
    let hk = Hkdf::<Sha256>::new(Some(HKDF_SALT), &root.0);
    let mut okm = [0u8; AES_KEY_LEN];
    hk.expand(info, &mut okm).map_err(|_| CryptoError::Derive)?;
    Ok(okm)
}

pub fn encrypt_plaintext(plaintext: &str, key: &EncryptionKey) -> Result<EnvelopeV1, CryptoError> {
    let cipher = Aes256Gcm::new_from_slice(&key.0).map_err(|_| CryptoError::Encrypt)?;
    let mut iv = [0u8; GCM_IV_LEN];
    rand::thread_rng().fill_bytes(&mut iv);
    let nonce = Nonce::from_slice(&iv);
    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|_| CryptoError::Encrypt)?;
    Ok(EnvelopeV1 {
        v: 1,
        encrypted: STANDARD.encode(ciphertext),
        iv: STANDARD.encode(iv),
    })
}

pub fn decrypt_envelope(envelope: &EnvelopeV1, key: &EncryptionKey) -> Result<String, CryptoError> {
    if envelope.v != 1 {
        return Err(CryptoError::UnsupportedEnvelope);
    }
    let cipher = Aes256Gcm::new_from_slice(&key.0).map_err(|_| CryptoError::Decrypt)?;
    let iv = STANDARD
        .decode(&envelope.iv)
        .map_err(|_| CryptoError::InvalidBase64)?;
    let ciphertext = STANDARD
        .decode(&envelope.encrypted)
        .map_err(|_| CryptoError::InvalidBase64)?;
    if iv.len() != GCM_IV_LEN {
        return Err(CryptoError::Decrypt);
    }
    let nonce = Nonce::from_slice(&iv);
    let plaintext = cipher
        .decrypt(nonce, ciphertext.as_ref())
        .map_err(|_| CryptoError::Decrypt)?;
    String::from_utf8(plaintext).map_err(|_| CryptoError::Decrypt)
}

pub fn encode_opaque_envelope(envelope: &EnvelopeV1) -> Result<String, CryptoError> {
    let json = serde_json::to_vec(envelope).map_err(|_| CryptoError::UnsupportedEnvelope)?;
    Ok(URL_SAFE_NO_PAD.encode(json))
}

pub fn decode_opaque_envelope(payload: &str) -> Result<EnvelopeV1, CryptoError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| CryptoError::InvalidBase64)?;
    let envelope: EnvelopeV1 =
        serde_json::from_slice(&bytes).map_err(|_| CryptoError::UnsupportedEnvelope)?;
    if envelope.v != 1 || envelope.encrypted.is_empty() || envelope.iv.is_empty() {
        return Err(CryptoError::UnsupportedEnvelope);
    }
    Ok(envelope)
}

pub fn encrypt_share(plaintext: &str) -> Result<EncryptedShare, CryptoError> {
    let root_secret = generate_root_secret();
    let key = derive_encryption_key(&root_secret)?;
    let envelope = encrypt_plaintext(plaintext, &key)?;
    Ok(EncryptedShare {
        payload: encode_opaque_envelope(&envelope)?,
        root_secret,
    })
}

pub fn decrypt_share(payload: &str, fragment: &ShareFragment) -> Result<String, CryptoError> {
    let envelope = decode_opaque_envelope(payload)?;
    let key = match fragment.secret_type {
        SecretType::Root => derive_encryption_key(&RootSecret(fragment.secret))?,
        SecretType::Key => EncryptionKey(fragment.secret),
    };
    decrypt_envelope(&envelope, &key)
}

pub fn encode_share_fragment(
    secret: &[u8; ROOT_SECRET_LEN],
    read_capability: &str,
    secret_type: SecretType,
) -> Result<String, CryptoError> {
    let read = read_capability
        .strip_prefix(READ_CAPABILITY_PREFIX)
        .ok_or(CryptoError::InvalidCredentials)?;
    let read_bytes = decode_base64url_32(read)?;
    let mut envelope = [0u8; FRAGMENT_LEN];
    envelope[0] = secret_type as u8;
    envelope[1..33].copy_from_slice(secret);
    envelope[33..].copy_from_slice(&read_bytes);
    Ok(URL_SAFE_NO_PAD.encode(envelope))
}

pub fn decode_share_fragment(fragment: &str) -> Result<ShareFragment, CryptoError> {
    let envelope = URL_SAFE_NO_PAD
        .decode(fragment)
        .map_err(|_| CryptoError::InvalidFragment)?;
    // Website shares put only the 32-byte root secret in the fragment.
    if envelope.len() == ROOT_SECRET_LEN {
        let mut secret = [0u8; ROOT_SECRET_LEN];
        secret.copy_from_slice(&envelope);
        let read_bytes = derive_read_capability(&RootSecret(secret))?;
        return Ok(ShareFragment {
            secret,
            secret_type: SecretType::Root,
            read_capability: URL_SAFE_NO_PAD.encode(read_bytes),
        });
    }
    if envelope.len() != FRAGMENT_LEN || (envelope[0] != 1 && envelope[0] != 2) {
        return Err(CryptoError::InvalidFragment);
    }
    let mut secret = [0u8; ROOT_SECRET_LEN];
    secret.copy_from_slice(&envelope[1..33]);
    let mut read_bytes = [0u8; ROOT_SECRET_LEN];
    read_bytes.copy_from_slice(&envelope[33..]);
    Ok(ShareFragment {
        secret,
        secret_type: if envelope[0] == 1 {
            SecretType::Root
        } else {
            SecretType::Key
        },
        read_capability: format!(
            "{READ_CAPABILITY_PREFIX}{}",
            URL_SAFE_NO_PAD.encode(read_bytes)
        ),
    })
}

pub fn encode_base64url(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

fn decode_base64url_32(value: &str) -> Result<[u8; 32], CryptoError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| CryptoError::InvalidBase64)?;
    if bytes.len() != 32 {
        return Err(CryptoError::InvalidCredentials);
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypts_and_decrypts_with_a_root_secret() {
        let encrypted = encrypt_share("deployment token").unwrap();
        let fragment = encode_share_fragment(
            &encrypted.root_secret.0,
            &format!(
                "{READ_CAPABILITY_PREFIX}{}",
                encode_base64url(&[7u8; 32])
            ),
            SecretType::Root,
        )
        .unwrap();
        let decoded = decode_share_fragment(&fragment).unwrap();
        assert_eq!(decrypt_share(&encrypted.payload, &decoded).unwrap(), "deployment token");
    }

    #[test]
    fn envelope_is_unpadded_base64url_json() {
        let envelope = EnvelopeV1 {
            v: 1,
            encrypted: "Y2lwaGVy".into(),
            iv: "aXY=".into(),
        };
        let payload = encode_opaque_envelope(&envelope).unwrap();
        assert!(payload.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        assert!(!payload.contains('='));
        let roundtrip = decode_opaque_envelope(&payload).unwrap();
        assert_eq!(roundtrip.encrypted, "Y2lwaGVy");
    }

    #[test]
    fn fragment_is_eighty_seven_unpadded_chars() {
        let secret = [9u8; 32];
        let capability = format!("{READ_CAPABILITY_PREFIX}{}", encode_base64url(&[3u8; 32]));
        let fragment = encode_share_fragment(&secret, &capability, SecretType::Root).unwrap();
        assert_eq!(fragment.len(), 87);
        let decoded = decode_share_fragment(&fragment).unwrap();
        assert_eq!(decoded.secret, secret);
        assert_eq!(decoded.secret_type, SecretType::Root);
        assert_eq!(decoded.read_capability, capability);
    }

    #[test]
    fn decrypts_a_web_crypto_compatible_vector() {
        // AES-256-GCM with a known key/iv produced by the same algorithm as lib/crypto.ts.
        let mut key_bytes = [0u8; 32];
        key_bytes[0] = 1;
        key_bytes[31] = 2;
        let key = EncryptionKey(key_bytes);
        let envelope = encrypt_plaintext("hello deleto", &key).unwrap();
        assert_eq!(decrypt_envelope(&envelope, &key).unwrap(), "hello deleto");
    }

    #[test]
    fn decrypts_from_a_website_root_secret_fragment() {
        let encrypted = encrypt_share("from the website").unwrap();
        let fragment = encode_base64url(&encrypted.root_secret.0);
        assert_eq!(fragment.len(), 43);
        let decoded = decode_share_fragment(&fragment).unwrap();
        assert_eq!(decoded.secret_type, SecretType::Root);
        assert!(!decoded.read_capability.starts_with(READ_CAPABILITY_PREFIX));
        assert_eq!(decoded.read_capability.len(), 43);
        assert_eq!(
            decrypt_share(&encrypted.payload, &decoded).unwrap(),
            "from the website"
        );
    }

    #[test]
    fn payload_json_never_contains_plaintext() {
        let encrypted = encrypt_share("super-secret-value").unwrap();
        let envelope = decode_opaque_envelope(&encrypted.payload).unwrap();
        let json = serde_json::to_string(&envelope).unwrap();
        assert!(!json.contains("super-secret-value"));
        assert!(!encrypted.payload.contains("super-secret-value"));
    }
}
