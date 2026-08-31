//! Client-side cryptography and URL helpers for the Deleto opaque API.
//!
//! The server stores an uninterpreted Base64URL envelope. Root secrets and
//! encryption keys never leave this process.

pub mod api;
pub mod crypto;
pub mod share;
pub mod share_url;

pub use api::{OpaqueClient, OPAQUE_MEDIA_TYPE};
pub use crypto::{decrypt_share, encrypt_share, EncryptedShare};
pub use share::{create_share, delete_share, view_share, CreateOptions, CreatedShare, ViewedShare};
pub use share_url::{build_share_url, parse_share_url, ShareLink};
