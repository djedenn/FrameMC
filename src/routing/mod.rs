//! Server routing and player connection lifecycle.

pub mod state_machine;

pub use state_machine::{
    extract_respawn_from_login, extract_respawn_from_login_with_data_kept,
    is_declare_commands_packet, is_login_play_packet, is_open_window_packet,
    is_serverbound_close_container_packet, is_sound_packet, AsyncStream, BackendConnector,
    BossBarPacket, CloseContainerPacket, DisplayObjectivePacket, PlayStateMachine, PlayerSession,
    RespawnPacket, ScoreboardObjectivePacket, ServerboundChatCommand, StopSoundPacket,
    SystemChatMessagePacket, TcpBackendConnector, BOSS_BAR_PACKET_ID, CLOSE_CONTAINER_PACKET_ID,
    DISPLAY_OBJECTIVE_PACKET_ID, KEEP_ALL_DATA, KEEP_ATTRIBUTES, KEEP_METADATA, RESPAWN_PACKET_ID,
    SCOREBOARD_OBJECTIVE_PACKET_ID, SERVERBOUND_CHAT_COMMAND_PACKET_ID,
    SERVERBOUND_CHAT_MESSAGE_PACKET_ID, STOP_SOUND_PACKET_ID, SYSTEM_CHAT_MESSAGE_PACKET_ID,
};
