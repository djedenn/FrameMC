use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use uuid::Uuid;

use crate::error::ProxyError;

/// Player profile property (e.g. skin textures, Cape URL) returned by Mojang session servers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileProperty {
    pub name: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

/// Authenticated player profile returned by Mojang session servers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerProfile {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub properties: Vec<ProfileProperty>,
}

/// Computes Mojang's custom SHA-1 digest for server authentication.
///
/// Layout:
/// - SHA-1 hash of `server_id + shared_secret + public_key_der`.
/// - Treat output as a two's-complement big-endian signed integer.
/// - If MSB is 1 (negative), compute two's complement and prepend with a `-`.
/// - Leading zeros are trimmed in hex representation.
pub fn mojang_sha1_digest(server_id: &str, shared_secret: &[u8], public_key_der: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(server_id.as_bytes());
    hasher.update(shared_secret);
    hasher.update(public_key_der);
    let mut hash: [u8; 20] = hasher.finalize().into();

    let is_negative = (hash[0] & 0x80) != 0;
    if is_negative {
        // Two's complement: invert bits, then add 1 with carry
        let mut carry = true;
        for byte in hash.iter_mut().rev() {
            *byte = !*byte;
            if carry {
                let (val, overflow) = (*byte).overflowing_add(1);
                *byte = val;
                carry = overflow;
            }
        }
        let hex_str = hex::encode(hash);
        let trimmed = hex_str.trim_start_matches('0');
        format!("-{}", if trimmed.is_empty() { "0" } else { trimmed })
    } else {
        let hex_str = hex::encode(hash);
        let trimmed = hex_str.trim_start_matches('0');
        if trimmed.is_empty() {
            "0".to_string()
        } else {
            trimmed.to_string()
        }
    }
}

pub const DEFAULT_MOJANG_SESSION_SERVER: &str =
    "https://sessionserver.mojang.com/session/minecraft/hasJoined";

/// Authenticates a client with Mojang's session servers via the `hasJoined` endpoint.
pub async fn verify_session(
    username: &str,
    server_hash: &str,
    client_ip: &str,
    client: &reqwest::Client,
) -> Result<PlayerProfile, ProxyError> {
    verify_session_with_url(
        username,
        server_hash,
        client_ip,
        client,
        DEFAULT_MOJANG_SESSION_SERVER,
    )
    .await
}

/// Authenticates a client using a custom or mock session server endpoint URL.
pub async fn verify_session_with_url(
    username: &str,
    server_hash: &str,
    _client_ip: &str,
    client: &reqwest::Client,
    endpoint_url: &str,
) -> Result<PlayerProfile, ProxyError> {
    let separator = if endpoint_url.contains('?') { "&" } else { "?" };
    // Following Velocity and Paper standards, do not send the optional `ip` parameter
    // as it triggers false-positive 204 No Content rejections on Mojang session servers
    // when connecting via localhost (127.0.0.1), LAN, VPN, or dual-stack IPv4/IPv6.
    let url = format!(
        "{endpoint_url}{separator}username={}&serverId={}",
        urlencoding(username),
        urlencoding(server_hash),
    );

    let response = client.get(&url).send().await.map_err(|e| {
        ProxyError::AuthenticationFailed(format!("Session server request failed: {e}"))
    })?;

    if response.status() == reqwest::StatusCode::NO_CONTENT {
        return Err(ProxyError::AuthenticationFailed(
            "Mojang session server returned 204 No Content (unverified session)".to_string(),
        ));
    }

    if !response.status().is_success() {
        return Err(ProxyError::AuthenticationFailed(format!(
            "Mojang session server returned HTTP status {}",
            response.status()
        )));
    }

    let profile: PlayerProfile = response.json().await.map_err(|e| {
        ProxyError::AuthenticationFailed(format!("Failed to parse session profile JSON: {e}"))
    })?;

    Ok(profile)
}

/// Generates an offline mode profile with an MD5-derived UUID.
pub fn offline_profile(username: &str) -> PlayerProfile {
    let id = Uuid::new_v3(
        &Uuid::NAMESPACE_OID,
        format!("OfflinePlayer:{}", username).as_bytes(),
    );
    PlayerProfile {
        id,
        name: username.to_string(),
        properties: Vec::new(),
    }
}

/// Minimal percent-encoding helper for query parameter safety.
fn urlencoding(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len());
    for b in input.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(b as char);
            }
            _ => {
                encoded.push_str(&format!("%{:02X}", b));
            }
        }
    }
    encoded
}

#[cfg(test)]
pub mod tests {
    use super::*;

    #[test]
    fn test_mojang_sha1_known_vectors() {
        // 1. "Notch" -> Positive hash (MSB = 0, no leading zero)
        assert_eq!(
            mojang_sha1_digest("Notch", b"", b""),
            "4ed1f46bbe04bc756bcb17c0c7ce3e4632f06a48"
        );

        // 2. "simon" -> Positive hash with leading zero trimmed (088e16... -> 88e16...)
        assert_eq!(
            mojang_sha1_digest("simon", b"", b""),
            "88e16a1019277b15d58faf0541e11910eb756f6"
        );

        // 3. "jeb_" -> Negative hash (MSB = 1, formats with leading minus sign)
        assert_eq!(
            mojang_sha1_digest("jeb_", b"", b""),
            "-7c9d5b0044c130109a5d7b5fb5c317c02b4e28c1"
        );
    }

    #[test]
    fn test_mojang_sha1_combined_components() {
        let server_id = "";
        let shared_secret = b"1234567890abcdef";
        let public_key_der = b"sample_public_key_bytes";

        let digest = mojang_sha1_digest(server_id, shared_secret, public_key_der);
        assert!(!digest.is_empty());
        // Verify output is valid hex or leading minus + hex
        let hex_part = digest.strip_prefix('-').unwrap_or(&digest);
        assert!(hex_part.chars().all(|c| c.is_ascii_hexdigit()));
        // Ensure leading zeros are trimmed
        assert!(!hex_part.starts_with('0') || hex_part == "0");
    }

    #[test]
    fn test_offline_profile_generation() {
        let profile = offline_profile("Steve");
        assert_eq!(profile.name, "Steve");
        assert_eq!(profile.properties.len(), 0);

        let expected_uuid = Uuid::new_v3(&Uuid::NAMESPACE_OID, b"OfflinePlayer:Steve");
        assert_eq!(profile.id, expected_uuid);

        let profile2 = offline_profile("Alex");
        assert_ne!(profile.id, profile2.id);
    }

    #[test]
    fn test_player_profile_json_deserialization() {
        let json_data = r#"{
            "id": "069a79f444e34726a9be254cc4d37b01",
            "name": "Steve",
            "properties": [
                {
                    "name": "textures",
                    "value": "eyJ0aW1lc3RhbXAiOjE3MTgwMDAwMDB9",
                    "signature": "signature_mock_data"
                }
            ]
        }"#;

        let profile: PlayerProfile =
            serde_json::from_str(json_data).expect("Failed to deserialize profile");
        assert_eq!(profile.name, "Steve");
        assert_eq!(
            profile.id,
            Uuid::parse_str("069a79f4-44e3-4726-a9be-254cc4d37b01").unwrap()
        );
        assert_eq!(profile.properties.len(), 1);
        assert_eq!(profile.properties[0].name, "textures");
        assert_eq!(
            profile.properties[0].value,
            "eyJ0aW1lc3RhbXAiOjE3MTgwMDAwMDB9"
        );
        assert_eq!(
            profile.properties[0].signature,
            Some("signature_mock_data".to_string())
        );
    }
}
