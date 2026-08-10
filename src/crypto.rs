//! Anonymous keying and at-rest encryption.
//!
//! Every page gets an X25519 keypair. The public key encrypts incoming private
//! recommendations (libsodium-style sealed boxes); the private key is stored
//! only wrapped with a key derived from the owner's secret, which the server
//! never persists. A DB dump therefore reveals nothing about private
//! recommendations.

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use crypto_box::{PublicKey, SecretKey};
use hmac::{Hmac, Mac};
use rand::rngs::OsRng;
use rand::RngCore;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

const WRAP_SALT: &[u8] = b"anonbookrec.keywrap.v1";
const NONCE_LEN: usize = 12;

/// 32 random bytes, base58 — the owner's secret ("anonymous key").
pub fn generate_secret() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    bs58::encode(bytes).into_string()
}

/// Short public page identifier for share URLs.
pub fn generate_slug() -> String {
    let mut bytes = [0u8; 8];
    OsRng.fill_bytes(&mut bytes);
    bs58::encode(bytes).into_string()
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

pub fn hash_secret(secret: &str) -> String {
    sha256_hex(secret.as_bytes())
}

/// Constant-time comparison of a presented secret against the stored hash.
pub fn verify_secret(secret: &str, stored_hash_hex: &str) -> bool {
    let presented = Sha256::digest(secret.as_bytes());
    let Ok(stored) = hex::decode(stored_hash_hex) else { return false };
    presented.ct_eq(stored.as_slice()).into()
}

fn wrap_key(secret: &str) -> ChaCha20Poly1305 {
    let hk = hkdf::Hkdf::<Sha256>::new(Some(WRAP_SALT), secret.as_bytes());
    let mut key = [0u8; 32];
    hk.expand(b"private-key-wrap", &mut key).expect("32 bytes is valid for hkdf");
    ChaCha20Poly1305::new((&key).into())
}

/// Generate a page keypair: plaintext public key + private key wrapped under
/// the owner's secret (random nonce prepended to the ciphertext).
pub fn new_page_keys(secret: &str) -> (Vec<u8>, Vec<u8>) {
    let private = SecretKey::generate(&mut OsRng);
    let public = private.public_key().as_bytes().to_vec();
    let wrapped = wrap_private_key(secret, &private);
    (public, wrapped)
}

/// Wrap an existing page private key under a (possibly new) owner secret.
/// Used on key rotation: the keypair stays the same so sealed payloads remain
/// readable, only the wrapping secret changes.
pub fn wrap_private_key(secret: &str, private: &SecretKey) -> Vec<u8> {
    let mut nonce = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce);
    let ciphertext = wrap_key(secret)
        .encrypt(&Nonce::from(nonce), private.to_bytes().as_slice())
        .expect("chacha20poly1305 encryption cannot fail");

    let mut wrapped = nonce.to_vec();
    wrapped.extend_from_slice(&ciphertext);
    wrapped
}

/// Recover the page's private key with the owner's secret. `None` on a wrong
/// secret or corrupted blob.
pub fn unwrap_private_key(secret: &str, wrapped: &[u8]) -> Option<SecretKey> {
    if wrapped.len() < NONCE_LEN {
        return None;
    }
    let (nonce, ciphertext) = wrapped.split_at(NONCE_LEN);
    let nonce: [u8; NONCE_LEN] = nonce.try_into().ok()?;
    let bytes = wrap_key(secret).decrypt(&Nonce::from(nonce), ciphertext).ok()?;
    SecretKey::from_slice(&bytes).ok()
}

/// Seal a payload to the page's public key (anonymous sender).
pub fn seal(public_key: &[u8], plaintext: &[u8]) -> anyhow::Result<Vec<u8>> {
    let key = PublicKey::from_slice(public_key)
        .map_err(|_| anyhow::anyhow!("invalid stored public key"))?;
    key.seal(&mut OsRng, plaintext).map_err(|_| anyhow::anyhow!("sealing failed"))
}

pub fn unseal(private_key: &SecretKey, ciphertext: &[u8]) -> Option<Vec<u8>> {
    private_key.unseal(ciphertext).ok()
}

/// Argon2id PHC hash for password *verification*. Distinct from the fast
/// SHA-256 path used for the high-entropy page secret: passwords are guessable
/// and need a slow hash.
pub fn hash_password(password: &str) -> anyhow::Result<String> {
    use argon2::password_hash::{PasswordHasher, SaltString};
    let salt = SaltString::generate(&mut OsRng);
    argon2::Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|e| anyhow::anyhow!("password hashing failed: {e}"))
}

pub fn verify_password(password: &str, phc: &str) -> bool {
    use argon2::password_hash::{PasswordHash, PasswordVerifier};
    let Ok(parsed) = PasswordHash::new(phc) else { return false };
    argon2::Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok()
}

fn password_key(password: &str, salt: &[u8; 16]) -> ChaCha20Poly1305 {
    let mut key = [0u8; 32];
    argon2::Argon2::default()
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .expect("argon2 key derivation cannot fail with valid lengths");
    ChaCha20Poly1305::new((&key).into())
}

/// Encrypt the page secret under a password-derived key:
/// `salt(16) ‖ nonce(12) ‖ ciphertext`. Password login decrypts this to
/// recover the secret — the secret stays the real credential.
pub fn wrap_secret_with_password(password: &str, secret: &str) -> Vec<u8> {
    let mut salt = [0u8; 16];
    OsRng.fill_bytes(&mut salt);
    let mut nonce = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce);
    let ciphertext = password_key(password, &salt)
        .encrypt(&Nonce::from(nonce), secret.as_bytes())
        .expect("chacha20poly1305 encryption cannot fail");

    let mut out = salt.to_vec();
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    out
}

pub fn unwrap_secret_with_password(password: &str, wrapped: &[u8]) -> Option<String> {
    if wrapped.len() < 16 + NONCE_LEN {
        return None;
    }
    let (salt, rest) = wrapped.split_at(16);
    let (nonce, ciphertext) = rest.split_at(NONCE_LEN);
    let salt: [u8; 16] = salt.try_into().ok()?;
    let nonce: [u8; NONCE_LEN] = nonce.try_into().ok()?;
    let bytes = password_key(password, &salt).decrypt(&Nonce::from(nonce), ciphertext).ok()?;
    String::from_utf8(bytes).ok()
}

/// Unforgeable token binding a draft page (slug + secret) to this server, so
/// page creation can wait for an explicit confirm request without any
/// server-side draft state: the token proves the pair was issued by us.
pub fn draft_token(pepper: &[u8; 32], slug: &str, secret: &str) -> String {
    let mut mac =
        <Hmac<Sha256> as Mac>::new_from_slice(pepper).expect("hmac accepts any key length");
    mac.update(format!("page-draft:{slug}:{}", hash_secret(secret)).as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

pub fn verify_draft_token(pepper: &[u8; 32], slug: &str, secret: &str, token: &str) -> bool {
    draft_token(pepper, slug, secret).as_bytes().ct_eq(token.as_bytes()).into()
}

/// Deterministic blind index over (page, ISBN) keyed by the server pepper, so
/// duplicate detection works without storing plaintext ISBNs for private rows.
pub fn blind_index(pepper: &[u8; 32], user_id: Option<i64>, isbn13: &str) -> String {
    let mut mac =
        <Hmac<Sha256> as Mac>::new_from_slice(pepper).expect("hmac accepts any key length");
    mac.update(format!("page:{}:isbn:{isbn13}", user_id.unwrap_or(0)).as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_verification_roundtrip() {
        let secret = generate_secret();
        let hash = hash_secret(&secret);
        assert!(verify_secret(&secret, &hash));
        assert!(!verify_secret("wrong", &hash));
    }

    #[test]
    fn key_wrap_roundtrip() {
        let secret = generate_secret();
        let (public, wrapped) = new_page_keys(&secret);
        assert_eq!(public.len(), 32);
        let private = unwrap_private_key(&secret, &wrapped).expect("correct secret unwraps");
        assert_eq!(private.public_key().as_bytes().to_vec(), public);
        assert!(unwrap_private_key("wrong-secret", &wrapped).is_none());
    }

    #[test]
    fn seal_unseal_roundtrip() {
        let secret = generate_secret();
        let (public, wrapped) = new_page_keys(&secret);
        let sealed = seal(&public, b"The Dispossessed").unwrap();
        assert_ne!(sealed, b"The Dispossessed");
        let private = unwrap_private_key(&secret, &wrapped).unwrap();
        assert_eq!(unseal(&private, &sealed).unwrap(), b"The Dispossessed");
    }

    #[test]
    fn password_verification_roundtrip() {
        let phc = hash_password("correct horse battery").unwrap();
        assert!(verify_password("correct horse battery", &phc));
        assert!(!verify_password("wrong", &phc));
    }

    #[test]
    fn password_secret_wrap_roundtrip() {
        let secret = generate_secret();
        let wrapped = wrap_secret_with_password("hunter2hunter2", &secret);
        assert_eq!(
            unwrap_secret_with_password("hunter2hunter2", &wrapped).as_deref(),
            Some(secret.as_str())
        );
        assert!(unwrap_secret_with_password("wrong-password", &wrapped).is_none());
    }

    #[test]
    fn blind_index_deterministic_and_scoped() {
        let pepper = [7u8; 32];
        let a = blind_index(&pepper, Some(1), "9780134685991");
        assert_eq!(a, blind_index(&pepper, Some(1), "9780134685991"));
        assert_ne!(a, blind_index(&pepper, Some(2), "9780134685991"));
        assert_ne!(a, blind_index(&pepper, None, "9780134685991"));
        assert_ne!(a, blind_index(&[8u8; 32], Some(1), "9780134685991"));
    }
}
