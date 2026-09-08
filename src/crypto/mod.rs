//! Cryptographic operations, key generation, and handshakes.

pub mod aes;
pub mod mojang_auth;

pub use aes::EncryptedStream;
pub use mojang_auth::{
    mojang_sha1_digest, offline_profile, verify_session, verify_session_with_url, PlayerProfile,
    ProfileProperty, DEFAULT_MOJANG_SESSION_SERVER,
};

use rand::rngs::OsRng;
use rsa::pkcs8::EncodePublicKey;
use rsa::{Pkcs1v15Encrypt, RsaPrivateKey, RsaPublicKey};
use zeroize::Zeroize;

use crate::error::ProxyError;

/// Manages the proxy's RSA 1024-bit session keypair.
#[derive(Clone)]
pub struct RsaKeyManager {
    private_key: Option<RsaPrivateKey>,
    public_key_der: Vec<u8>,
}

impl Drop for RsaKeyManager {
    fn drop(&mut self) {
        self.zeroize();
    }
}

impl Zeroize for RsaKeyManager {
    fn zeroize(&mut self) {
        self.public_key_der.zeroize();
        if let Some(key) = self.private_key.take() {
            drop(key); // Invokes RsaPrivateKey::drop (ZeroizeOnDrop), clearing d, primes, and precomputed values
        }
    }
}

impl RsaKeyManager {
    /// Generates a new 1024-bit RSA keypair and pre-encodes the public key in ASN.1 DER format.
    pub fn new() -> Result<Self, ProxyError> {
        let mut rng = OsRng;
        let private_key = RsaPrivateKey::new(&mut rng, 1024)
            .map_err(|e| ProxyError::CryptoError(format!("Failed to generate RSA keypair: {e}")))?;

        let public_key = RsaPublicKey::from(&private_key);
        let public_key_der = public_key
            .to_public_key_der()
            .map_err(|e| {
                ProxyError::CryptoError(format!("Failed to export public key to DER: {e}"))
            })?
            .as_bytes()
            .to_vec();

        Ok(Self {
            private_key: Some(private_key),
            public_key_der,
        })
    }

    /// Returns the ASN.1 DER-encoded SubjectPublicKeyInfo bytes.
    pub fn public_key_der(&self) -> &[u8] {
        &self.public_key_der
    }

    /// Decrypts a PKCS1v15-padded RSA ciphertext using the private key.
    pub fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>, ProxyError> {
        let private_key = self.private_key.as_ref().ok_or_else(|| {
            ProxyError::CryptoError("RSA private key has been zeroized".to_string())
        })?;
        private_key
            .decrypt(Pkcs1v15Encrypt, ciphertext)
            .map_err(|e| ProxyError::CryptoError(format!("RSA decryption failed: {e}")))
    }

    /// Encrypts plaintext using PKCS1v15 padding and the associated public key.
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, ProxyError> {
        let private_key = self.private_key.as_ref().ok_or_else(|| {
            ProxyError::CryptoError("RSA private key has been zeroized".to_string())
        })?;
        let mut rng = OsRng;
        let public_key = RsaPublicKey::from(private_key);
        public_key
            .encrypt(&mut rng, Pkcs1v15Encrypt, plaintext)
            .map_err(|e| ProxyError::CryptoError(format!("RSA encryption failed: {e}")))
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use rand::RngCore;

    #[test]
    fn test_rsa_key_manager_der_export() {
        let manager = RsaKeyManager::new().expect("Failed to initialize RsaKeyManager");
        let der = manager.public_key_der();

        // Valid ASN.1 SEQUENCE begins with 0x30
        assert!(!der.is_empty());
        assert_eq!(
            der[0], 0x30,
            "DER export must start with ASN.1 SEQUENCE (0x30)"
        );
        // 1024-bit RSA public key DER SubjectPublicKeyInfo is typically around 162 bytes
        assert!(
            der.len() > 100 && der.len() < 200,
            "Unexpected DER length: {}",
            der.len()
        );
    }

    #[test]
    fn test_rsa_encrypt_decrypt_roundtrip_16_bytes() {
        let manager = RsaKeyManager::new().expect("Failed to initialize RsaKeyManager");

        let mut token = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut token);

        let ciphertext = manager.encrypt(&token).expect("Failed to encrypt token");
        assert_ne!(&ciphertext[..], &token[..]);

        let decrypted = manager
            .decrypt(&ciphertext)
            .expect("Failed to decrypt ciphertext");
        assert_eq!(
            &decrypted[..],
            &token[..],
            "Decrypted token must match original"
        );
    }
}
