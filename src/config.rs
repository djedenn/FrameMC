use crate::error::ProxyError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Supported backend player information forwarding modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForwardingMode {
    #[serde(alias = "none", alias = "None", alias = "NONE")]
    None,
    #[serde(
        alias = "legacy_bungee",
        alias = "LegacyBungee",
        alias = "legacy",
        alias = "bungee"
    )]
    LegacyBungee,
    #[serde(
        alias = "velocity_modern",
        alias = "VelocityModern",
        alias = "velocity",
        alias = "modern"
    )]
    VelocityModern,
}

/// Configuration settings for an individual backend server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendConfig {
    pub address: String,
    pub port: u16,
    pub forwarding_mode: ForwardingMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forwarding_secret: Option<String>,
}

/// Top-level configuration for the FrameMC proxy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyConfig {
    pub bind_address: String,
    pub bind_port: u16,
    pub motd: String,
    pub max_players: i32,
    pub online_mode: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub favicon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_server_url: Option<String>,
    pub servers: HashMap<String, BackendConfig>,
    pub default_server: String,
    pub script_path: String,
    #[serde(default = "default_plugins_dir")]
    pub plugins_dir: String,
}

fn default_plugins_dir() -> String {
    "plugins".to_string()
}

pub const DEFAULT_CONFIG_TEMPLATE: &str = r#"# =============================================================================
# FrameMC - High-Performance Minecraft Proxy Configuration
# =============================================================================

# Network binding configuration
bind_address = "0.0.0.0"
bind_port = 25565

# Server List Ping (Status) appearance
motd = "§aFrameMC §7High-Performance Minecraft Proxy"
max_players = 1000

# Authentication mode
# true  = Authenticate player accounts via Mojang session servers
# false = Offline mode / no external authentication
online_mode = true

# Optional path or base64 data URI for server favicon (64x64 PNG)
# favicon = "server-icon.png"

# Default backend server for newly connecting clients
default_server = "lobby"

# Directory containing Rhai plugin scripts (*.rhai)
plugins_dir = "plugins"

# Path to the Rhai scripting file for event interception
script_path = "scripts/main.rhai"

# =============================================================================
# Backend Server Definitions
# =============================================================================
# Supported forwarding modes:
# - "none": No player info forwarding
# - "legacy_bungee": BungeeCord handshake appending (client_host\0ip\0uuid)
# - "velocity_modern": Velocity modern forwarding via velocity:player_info (HMAC-SHA256)

[servers.lobby]
address = "127.0.0.1"
port = 25566
forwarding_mode = "none"

[servers.steelmc]
address = "127.0.0.1"
port = 25567
forwarding_mode = "none"
"#;

impl Default for ProxyConfig {
    fn default() -> Self {
        let mut servers = HashMap::new();
        servers.insert(
            "lobby".to_string(),
            BackendConfig {
                address: "127.0.0.1".to_string(),
                port: 25566,
                forwarding_mode: ForwardingMode::None,
                forwarding_secret: None,
            },
        );
        servers.insert(
            "steelmc".to_string(),
            BackendConfig {
                address: "127.0.0.1".to_string(),
                port: 25567,
                forwarding_mode: ForwardingMode::None,
                forwarding_secret: None,
            },
        );

        Self {
            bind_address: "0.0.0.0".to_string(),
            bind_port: 25565,
            motd: "§aFrameMC §7High-Performance Minecraft Proxy".to_string(),
            max_players: 1000,
            online_mode: true,
            favicon: None,
            session_server_url: None,
            servers,
            default_server: "lobby".to_string(),
            script_path: "scripts/main.rhai".to_string(),
            plugins_dir: "plugins".to_string(),
        }
    }
}

impl ProxyConfig {
    /// Resolves the favicon into a base64 Data URI ("data:image/png;base64,...").
    pub fn resolve_favicon(&mut self, base_dir: Option<&Path>) {
        self.favicon = resolve_favicon_uri(self.favicon.as_deref(), base_dir);
    }

    /// Loads configuration from the given file path.
    /// If the file does not exist, creates it with a fully documented default configuration.
    pub fn load_or_create(path: &str) -> Result<Self, ProxyError> {
        let file_path = Path::new(path);
        let mut config: ProxyConfig = if file_path.exists() {
            let content = std::fs::read_to_string(file_path)?;
            toml::from_str(&content).map_err(|e| {
                ProxyError::ConfigError(format!("Failed to parse config file '{path}': {e}"))
            })?
        } else {
            if let Some(parent) = file_path.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)?;
                }
            }
            std::fs::write(file_path, DEFAULT_CONFIG_TEMPLATE)?;
            toml::from_str(DEFAULT_CONFIG_TEMPLATE).map_err(|e| {
                ProxyError::ConfigError(format!("Failed to parse default config template: {e}"))
            })?
        };

        config.resolve_favicon(file_path.parent());
        Ok(config)
    }
}

/// Resolves a favicon option or file path into a data URI (`data:image/png;base64,...`).
///
/// Behavior:
/// 1. If `favicon` is already a data URI (`data:image/png;base64,...`), returns it trimmed.
/// 2. If `favicon` is a path to a file (relative to `base_dir` or current working directory), reads
///    and encodes the PNG bytes into `data:image/png;base64,<base64>`.
/// 3. If `favicon` is `None`, auto-detects `server-icon.png` in `base_dir` or current working directory,
///    and if found, encodes it into `data:image/png;base64,<base64>`.
pub fn resolve_favicon_uri(favicon: Option<&str>, base_dir: Option<&Path>) -> Option<String> {
    use base64::prelude::*;

    if let Some(val) = favicon {
        let val_trimmed = val.trim();
        if val_trimmed.starts_with("data:image/png;base64,") {
            return Some(val_trimmed.to_string());
        }

        // Try relative to base_dir
        if let Some(dir) = base_dir {
            let path = dir.join(val_trimmed);
            if path.is_file() {
                if let Ok(bytes) = std::fs::read(&path) {
                    let encoded = BASE64_STANDARD.encode(&bytes);
                    tracing::info!("Loaded server favicon from '{}'", path.display());
                    return Some(format!("data:image/png;base64,{}", encoded));
                }
            }
        }

        // Try relative to current working directory
        let path = Path::new(val_trimmed);
        if path.is_file() {
            if let Ok(bytes) = std::fs::read(path) {
                let encoded = BASE64_STANDARD.encode(&bytes);
                tracing::info!("Loaded server favicon from '{}'", path.display());
                return Some(format!("data:image/png;base64,{}", encoded));
            }
        }

        // Try relative to executable directory
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                let path = exe_dir.join(val_trimmed);
                if path.is_file() {
                    if let Ok(bytes) = std::fs::read(&path) {
                        let encoded = BASE64_STANDARD.encode(&bytes);
                        tracing::info!("Loaded server favicon from '{}'", path.display());
                        return Some(format!("data:image/png;base64,{}", encoded));
                    }
                }
            }
        }

        // If it's already a raw base64 string without data URI prefix
        if val_trimmed.len() > 64 && BASE64_STANDARD.decode(val_trimmed).is_ok() {
            return Some(format!("data:image/png;base64,{}", val_trimmed));
        }

        tracing::warn!(
            "Specified favicon '{}' was not found or could not be read",
            val_trimmed
        );
        None
    } else {
        // Auto-detect server-icon.png if favicon is None
        if let Some(dir) = base_dir {
            let path = dir.join("server-icon.png");
            if path.is_file() {
                if let Ok(bytes) = std::fs::read(&path) {
                    let encoded = BASE64_STANDARD.encode(&bytes);
                    tracing::info!(
                        "Auto-detected and loaded server favicon from '{}'",
                        path.display()
                    );
                    return Some(format!("data:image/png;base64,{}", encoded));
                }
            }
        }

        let path = Path::new("server-icon.png");
        if path.is_file() {
            if let Ok(bytes) = std::fs::read(path) {
                let encoded = BASE64_STANDARD.encode(&bytes);
                tracing::info!(
                    "Auto-detected and loaded server favicon from '{}'",
                    path.display()
                );
                return Some(format!("data:image/png;base64,{}", encoded));
            }
        }

        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                let path = exe_dir.join("server-icon.png");
                if path.is_file() {
                    if let Ok(bytes) = std::fs::read(&path) {
                        let encoded = BASE64_STANDARD.encode(&bytes);
                        tracing::info!(
                            "Auto-detected and loaded server favicon from '{}'",
                            path.display()
                        );
                        return Some(format!("data:image/png;base64,{}", encoded));
                    }
                }
            }
        }

        None
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    #[test]
    fn test_forwarding_mode_parsing() {
        // Test snake_case parsing
        let mode_none: ForwardingMode = toml::from_str("mode = \"none\"")
            .map(|v: HashMap<String, ForwardingMode>| v["mode"])
            .expect("Failed to parse 'none'");
        assert_eq!(mode_none, ForwardingMode::None);

        let mode_legacy: ForwardingMode = toml::from_str("mode = \"legacy_bungee\"")
            .map(|v: HashMap<String, ForwardingMode>| v["mode"])
            .expect("Failed to parse 'legacy_bungee'");
        assert_eq!(mode_legacy, ForwardingMode::LegacyBungee);

        let mode_velocity: ForwardingMode = toml::from_str("mode = \"velocity_modern\"")
            .map(|v: HashMap<String, ForwardingMode>| v["mode"])
            .expect("Failed to parse 'velocity_modern'");
        assert_eq!(mode_velocity, ForwardingMode::VelocityModern);

        // Test PascalCase aliases
        let mode_none_pascal: ForwardingMode = toml::from_str("mode = \"None\"")
            .map(|v: HashMap<String, ForwardingMode>| v["mode"])
            .expect("Failed to parse 'None'");
        assert_eq!(mode_none_pascal, ForwardingMode::None);

        let mode_legacy_pascal: ForwardingMode = toml::from_str("mode = \"LegacyBungee\"")
            .map(|v: HashMap<String, ForwardingMode>| v["mode"])
            .expect("Failed to parse 'LegacyBungee'");
        assert_eq!(mode_legacy_pascal, ForwardingMode::LegacyBungee);

        let mode_velocity_pascal: ForwardingMode = toml::from_str("mode = \"VelocityModern\"")
            .map(|v: HashMap<String, ForwardingMode>| v["mode"])
            .expect("Failed to parse 'VelocityModern'");
        assert_eq!(mode_velocity_pascal, ForwardingMode::VelocityModern);
    }

    #[test]
    fn test_toml_roundtrip() {
        let mut servers = HashMap::new();
        servers.insert(
            "lobby".to_string(),
            BackendConfig {
                address: "127.0.0.1".to_string(),
                port: 25566,
                forwarding_mode: ForwardingMode::VelocityModern,
                forwarding_secret: Some("secret_vel_123".to_string()),
            },
        );
        servers.insert(
            "bungee_fallback".to_string(),
            BackendConfig {
                address: "192.168.1.50".to_string(),
                port: 25568,
                forwarding_mode: ForwardingMode::LegacyBungee,
                forwarding_secret: None,
            },
        );
        servers.insert(
            "offline_dev".to_string(),
            BackendConfig {
                address: "10.0.0.1".to_string(),
                port: 25569,
                forwarding_mode: ForwardingMode::None,
                forwarding_secret: None,
            },
        );

        let original = ProxyConfig {
            bind_address: "127.0.0.1".to_string(),
            bind_port: 25565,
            motd: "§bCustom Proxy MOTD".to_string(),
            max_players: 500,
            online_mode: false,
            favicon: Some("data:image/png;base64,iVBORw0KGgoAAAANSUhEUg==".to_string()),
            session_server_url: None,
            servers,
            default_server: "lobby".to_string(),
            script_path: "scripts/intercept.rhai".to_string(),
            plugins_dir: "plugins".to_string(),
        };

        let toml_string =
            toml::to_string_pretty(&original).expect("Failed to serialize ProxyConfig");
        let deserialized: ProxyConfig =
            toml::from_str(&toml_string).expect("Failed to deserialize ProxyConfig");

        assert_eq!(original, deserialized);
    }

    #[test]
    fn test_default_template_matches_default_struct() {
        let parsed_template: ProxyConfig = toml::from_str(DEFAULT_CONFIG_TEMPLATE)
            .expect("Failed to parse default config template");
        let default_struct = ProxyConfig::default();

        assert_eq!(parsed_template, default_struct);
        assert_eq!(parsed_template.servers.len(), 2);
        assert_eq!(parsed_template.servers["lobby"].address, "127.0.0.1");
        assert_eq!(parsed_template.servers["lobby"].port, 25566);
        assert_eq!(parsed_template.servers["steelmc"].address, "127.0.0.1");
        assert_eq!(parsed_template.servers["steelmc"].port, 25567);
    }

    #[test]
    fn test_load_or_create() {
        let temp_dir = std::env::temp_dir();
        let unique_name = format!("framemc_test_config_{}.toml", rand::random::<u32>());
        let temp_path = temp_dir.join(unique_name);
        let path_str = temp_path.to_str().unwrap();

        // 1. File does not exist -> creates and loads
        assert!(!temp_path.exists());
        let config1 =
            ProxyConfig::load_or_create(path_str).expect("Failed to load_or_create new file");
        assert!(temp_path.exists());
        assert_eq!(config1.default_server, "lobby");
        assert_eq!(config1.servers["lobby"].port, 25566);
        assert_eq!(config1.servers["steelmc"].port, 25567);

        // 2. File exists -> loads existing file
        let config2 = ProxyConfig::load_or_create(path_str).expect("Failed to load existing file");
        assert_eq!(config1, config2);

        // Clean up
        let _ = std::fs::remove_file(temp_path);
    }

    #[test]
    fn test_resolve_favicon_uri_data_uri() {
        let uri = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUg==";
        let resolved = resolve_favicon_uri(Some(uri), None);
        assert_eq!(resolved, Some(uri.to_string()));
    }

    #[test]
    fn test_resolve_favicon_uri_file_path() {
        let temp_dir = std::env::temp_dir();
        let icon_path = temp_dir.join("test-icon.png");
        let sample_png_bytes = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00@\x00\x00\x00@";
        std::fs::write(&icon_path, sample_png_bytes).unwrap();

        let resolved = resolve_favicon_uri(Some("test-icon.png"), Some(&temp_dir));
        assert!(resolved.is_some());
        let uri = resolved.unwrap();
        assert!(uri.starts_with("data:image/png;base64,"));
        let b64 = &uri["data:image/png;base64,".len()..];
        use base64::prelude::*;
        let decoded = BASE64_STANDARD.decode(b64).unwrap();
        assert_eq!(decoded, sample_png_bytes);

        let _ = std::fs::remove_file(icon_path);
    }

    #[test]
    fn test_resolve_favicon_uri_auto_detect() {
        let temp_dir =
            std::env::temp_dir().join(format!("framemc_icon_test_{}", rand::random::<u32>()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let icon_path = temp_dir.join("server-icon.png");
        let sample_bytes = b"\x89PNG\r\n\x1a\nauto_detected";
        std::fs::write(&icon_path, sample_bytes).unwrap();

        let resolved = resolve_favicon_uri(None, Some(&temp_dir));
        assert!(resolved.is_some());
        let uri = resolved.unwrap();
        assert!(uri.starts_with("data:image/png;base64,"));

        let _ = std::fs::remove_file(icon_path);
        let _ = std::fs::remove_dir(temp_dir);
    }

    #[test]
    fn test_load_or_create_nested_subdirectory_cross_platform() {
        let temp_dir = std::env::temp_dir();
        let unique_folder = format!("framemc_nested_{}", rand::random::<u32>());
        let nested_dir = temp_dir.join(unique_folder).join("nested_sub");
        let nested_config_path = nested_dir.join("proxy_conf.toml");
        let path_str = nested_config_path.to_str().unwrap();

        assert!(!nested_config_path.exists());
        let config =
            ProxyConfig::load_or_create(path_str).expect("Failed to load_or_create nested");
        assert!(nested_config_path.exists());
        assert_eq!(config.default_server, "lobby");

        let _ = std::fs::remove_file(&nested_config_path);
        let _ = std::fs::remove_dir_all(nested_dir.parent().unwrap());
    }
}
