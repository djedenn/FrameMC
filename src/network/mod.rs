//! Network handling module for FrameMC proxy.

pub mod bridge;
pub mod codec;
pub mod listener;

pub use bridge::bridge_play_streams;
pub use codec::{
    encode_packet_with_compression, read_packet, read_packet_with_compression, write_packet,
    write_packet_with_compression, DisconnectPacket, SetCompressionPacket, DEFAULT_MAX_PACKET_SIZE,
};
pub use listener::{bind_listener, start_listener, start_listener_on};
