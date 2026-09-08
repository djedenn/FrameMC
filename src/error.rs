use thiserror::Error;

#[derive(Error, Debug)]
pub enum ProxyError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("VarInt exceeds maximum allowed length")]
    VarIntOverflow,

    #[error("Packet size {0} exceeds configured maximum limit")]
    PacketTooLarge(usize),

    #[error("Invalid or unrecognized packet ID: {0:#x}")]
    InvalidPacketId(i32),

    #[error("Invalid connection state: {0}")]
    InvalidConnectionState(i32),

    #[error("Cryptographic error: {0}")]
    CryptoError(String),

    #[error("Authentication failed: {0}")]
    AuthenticationFailed(String),

    #[error("Backend connection failed: {0}")]
    BackendConnectionFailed(String),

    #[error("Script error: {0}")]
    ScriptError(String),

    #[error("Configuration error: {0}")]
    ConfigError(String),
}

pub type Result<T> = std::result::Result<T, ProxyError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = ProxyError::VarIntOverflow;
        assert_eq!(err.to_string(), "VarInt exceeds maximum allowed length");

        let err = ProxyError::PacketTooLarge(2097152);
        assert_eq!(
            err.to_string(),
            "Packet size 2097152 exceeds configured maximum limit"
        );

        let err = ProxyError::InvalidPacketId(0x02);
        assert_eq!(err.to_string(), "Invalid or unrecognized packet ID: 0x2");

        let err = ProxyError::InvalidConnectionState(99);
        assert_eq!(err.to_string(), "Invalid connection state: 99");

        let err = ProxyError::CryptoError("bad key".into());
        assert_eq!(err.to_string(), "Cryptographic error: bad key");

        let err = ProxyError::AuthenticationFailed("invalid hash".into());
        assert_eq!(err.to_string(), "Authentication failed: invalid hash");

        let err = ProxyError::BackendConnectionFailed("timeout".into());
        assert_eq!(err.to_string(), "Backend connection failed: timeout");

        let err = ProxyError::ScriptError("stack overflow".into());
        assert_eq!(err.to_string(), "Script error: stack overflow");

        let err = ProxyError::ConfigError("missing host".into());
        assert_eq!(err.to_string(), "Configuration error: missing host");

        let io_err = std::io::Error::new(std::io::ErrorKind::ConnectionReset, "reset");
        let err = ProxyError::from(io_err);
        assert!(err.to_string().contains("reset"));
    }
}
