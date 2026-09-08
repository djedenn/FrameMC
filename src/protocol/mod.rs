//! Minecraft protocol primitives and packet codecs.

pub mod configuration;
pub mod forwarding;
pub mod handshake;
pub mod login;
pub mod packet;
pub mod status;
pub mod varint;

pub use configuration::{
    ClientboundConfigPacket, ConfigAction, FinishConfigurationPacket, RegistryDataPacket,
    RegistryEntry, SessionRegistryCache, FINISH_CONFIGURATION_PACKET_ID, REGISTRY_DATA_PACKET_ID,
};

pub use forwarding::{
    connect_and_forward, create_bungeecord_handshake_address, create_velocity_forwarding_payload,
    create_velocity_response_data, dispatch_forwarding, LoginPluginRequestPacket,
    LoginPluginResponsePacket, VELOCITY_FORWARDING_CHANNEL, VELOCITY_FORWARDING_VERSION,
};
pub use handshake::{ConnectionState, HandshakePacket};
pub use login::{
    encode_frame_packet, encode_login_success, encode_login_success_frame,
    encode_login_success_frame_with_session, encode_login_success_with_session,
    EncryptionRequestPacket, EncryptionResponsePacket, LoginAcknowledgedPacket, LoginStartPacket,
    LoginSuccessPacket,
};
pub use packet::{is_protected_plugin_channel, RawPacket};
pub use status::{
    handle_status, handle_status_with_version, ChatDescription, PlayerSample, PlayersInfo,
    StatusResponse, VersionInfo,
};
