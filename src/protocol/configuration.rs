use bytes::{Buf, BufMut, Bytes, BytesMut};
use std::collections::BTreeMap;
use tokio::io::AsyncWrite;

use crate::error::ProxyError;
use crate::network::codec::write_packet;
use crate::protocol::handshake::ConnectionState;
use crate::protocol::packet::RawPacket;
use crate::protocol::varint::{decode_varint, encode_varint};

/// Packet ID for `RegistryData` in modern Minecraft Configuration state.
pub const REGISTRY_DATA_PACKET_ID: i32 = 0x07;

/// Packet ID for clientbound `FinishConfiguration` in modern Minecraft Configuration state.
pub const FINISH_CONFIGURATION_PACKET_ID: i32 = 0x02;

/// A single entry within a registry data payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryEntry {
    pub entry_id: String,
    pub data: Option<Bytes>,
}

impl RegistryEntry {
    pub fn new(entry_id: impl Into<String>, data: Option<impl Into<Bytes>>) -> Self {
        Self {
            entry_id: entry_id.into(),
            data: data.map(Into::into),
        }
    }
}

/// Client-bound `RegistryData` packet (Packet ID: `0x07`).
///
/// Contains the registry identifier (e.g. `"minecraft:dimension_type"`, `"minecraft:worldgen/biome"`,
/// `"minecraft:damage_type"`) and the raw NBT bytes / entries.
/// Raw bytes are preserved without deserialization to avoid corruption and maintain version decoupling [R-06], [R-10].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryDataPacket {
    pub registry_id: String,
    pub raw_data: Bytes,
}

impl RegistryDataPacket {
    pub fn new(registry_id: impl Into<String>, raw_data: impl Into<Bytes>) -> Self {
        Self {
            registry_id: registry_id.into(),
            raw_data: raw_data.into(),
        }
    }

    /// Decodes a `RegistryDataPacket` from a `RawPacket`.
    ///
    /// Validates that:
    /// - `packet.id == 0x07`
    /// - `registry_id` is a valid VarInt-prefixed UTF-8 string
    /// - The remainder of the payload is captured verbatim as `raw_data`.
    pub fn decode(packet: &RawPacket) -> Result<Self, ProxyError> {
        if packet.id != REGISTRY_DATA_PACKET_ID {
            return Err(ProxyError::InvalidPacketId(packet.id));
        }

        let mut cursor = &packet.payload[..];

        // 1. Registry Identifier (VarInt length + UTF-8 string)
        let reg_id_len = decode_varint(&mut cursor)?;
        if reg_id_len < 0 || cursor.remaining() < reg_id_len as usize {
            return Err(ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "unexpected EOF reading registry_id",
            )));
        }

        let reg_id_bytes = &cursor[..reg_id_len as usize];
        let registry_id = std::str::from_utf8(reg_id_bytes)
            .map_err(|_| {
                ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid UTF-8 in registry_id",
                ))
            })?
            .to_string();
        cursor.advance(reg_id_len as usize);

        // 2. Raw NBT / entries payload
        let raw_data = Bytes::copy_from_slice(cursor);

        Ok(Self {
            registry_id,
            raw_data,
        })
    }

    /// Encodes this `RegistryDataPacket` into a `RawPacket` (Packet ID: `0x07`).
    pub fn encode(&self) -> RawPacket {
        let mut payload = BytesMut::new();
        encode_varint(self.registry_id.len() as i32, &mut payload);
        payload.put_slice(self.registry_id.as_bytes());
        payload.put_slice(&self.raw_data);
        RawPacket::new(REGISTRY_DATA_PACKET_ID, payload.freeze())
    }

    /// Helper to construct a `RegistryDataPacket` from structured `RegistryEntry` items
    /// following the Minecraft 1.20.5+ format:
    /// `[Entry Count: VarInt] { [Entry ID: String] [Has Data: u8] [NBT Data if Has Data == 1] }*`
    pub fn from_entries(registry_id: impl Into<String>, entries: &[RegistryEntry]) -> Self {
        let mut raw = BytesMut::new();
        encode_varint(entries.len() as i32, &mut raw);
        for entry in entries {
            encode_varint(entry.entry_id.len() as i32, &mut raw);
            raw.put_slice(entry.entry_id.as_bytes());
            if let Some(ref data) = entry.data {
                raw.put_u8(1);
                raw.put_slice(data);
            } else {
                raw.put_u8(0);
            }
        }
        Self::new(registry_id, raw.freeze())
    }

    /// Attempts to parse `raw_data` as a list of entries formatted per Minecraft 1.20.5+.
    pub fn try_decode_entries(&self) -> Result<Vec<RegistryEntry>, ProxyError> {
        let mut cursor = &self.raw_data[..];
        let count = decode_varint(&mut cursor)?;
        if count < 0 {
            return Err(ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "negative entry count",
            )));
        }

        let mut entries = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let e_len = decode_varint(&mut cursor)?;
            if e_len < 0 || cursor.remaining() < e_len as usize {
                return Err(ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "unexpected EOF reading entry_id",
                )));
            }
            let entry_id = std::str::from_utf8(&cursor[..e_len as usize])
                .map_err(|_| {
                    ProxyError::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "invalid UTF-8 in entry_id",
                    ))
                })?
                .to_string();
            cursor.advance(e_len as usize);

            if !cursor.has_remaining() {
                return Err(ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "unexpected EOF reading has_data flag",
                )));
            }
            let has_data = cursor.get_u8() != 0;
            let data = if has_data {
                Some(Bytes::copy_from_slice(cursor))
            } else {
                None
            };

            entries.push(RegistryEntry { entry_id, data });
            if has_data {
                break;
            }
        }
        Ok(entries)
    }
}

/// Client-bound `FinishConfiguration` packet (Packet ID: `0x02`).
///
/// Signals that configuration is complete and transitions the connection state to `Play`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FinishConfigurationPacket;

impl FinishConfigurationPacket {
    pub fn new() -> Self {
        Self
    }

    /// Decodes a `FinishConfigurationPacket` from a `RawPacket`.
    pub fn decode(packet: &RawPacket) -> Result<Self, ProxyError> {
        if packet.id != FINISH_CONFIGURATION_PACKET_ID {
            return Err(ProxyError::InvalidPacketId(packet.id));
        }
        Ok(Self)
    }

    /// Encodes this `FinishConfigurationPacket` into a `RawPacket` (Packet ID: `0x02`).
    pub fn encode(&self) -> RawPacket {
        RawPacket::new(FINISH_CONFIGURATION_PACKET_ID, Bytes::new())
    }

    /// Returns the target connection state following this packet (`ConnectionState::Play`).
    pub fn next_state(&self) -> ConnectionState {
        ConnectionState::Play
    }
}

/// Represents known or unparsed client-bound Configuration state packets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientboundConfigPacket {
    RegistryData(RegistryDataPacket),
    FinishConfiguration(FinishConfigurationPacket),
    Other(RawPacket),
}

impl ClientboundConfigPacket {
    pub fn decode(packet: &RawPacket) -> Result<Self, ProxyError> {
        match packet.id {
            REGISTRY_DATA_PACKET_ID => Ok(Self::RegistryData(RegistryDataPacket::decode(packet)?)),
            FINISH_CONFIGURATION_PACKET_ID => Ok(Self::FinishConfiguration(
                FinishConfigurationPacket::decode(packet)?,
            )),
            _ => Ok(Self::Other(packet.clone())),
        }
    }
}

/// Action resulting from processing a client-bound configuration packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigAction {
    /// Intercepted and cached a `RegistryData` packet with the specified registry identifier.
    CachedRegistry(String),
    /// Received `FinishConfiguration`, connection must transition to `ConnectionState::Play`.
    TransitionToPlay,
    /// Uninspected configuration packet that should be passed through.
    Passthrough(RawPacket),
}

/// In-memory session cache for raw registry data packets intercepted during Configuration state.
///
/// Ensures compliance with [R-10]: dimensions (`minecraft:dimension_type`), biomes
/// (`minecraft:worldgen/biome`), and damage types (`minecraft:damage_type`) are stored verbatim
/// and can be serialized or replayed to the client when transferring between backend servers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionRegistryCache {
    registries: BTreeMap<String, RegistryDataPacket>,
}

impl SessionRegistryCache {
    pub fn new() -> Self {
        Self {
            registries: BTreeMap::new(),
        }
    }

    /// Inserts a `RegistryDataPacket` into the cache.
    pub fn insert(&mut self, packet: RegistryDataPacket) {
        self.registries.insert(packet.registry_id.clone(), packet);
    }

    /// Inserts raw registry NBT bytes directly into the cache.
    pub fn insert_raw(&mut self, registry_id: impl Into<String>, raw_data: impl Into<Bytes>) {
        let packet = RegistryDataPacket::new(registry_id, raw_data);
        self.insert(packet);
    }

    /// Intercepts a raw packet: if it is a `RegistryData` packet (`0x07`),
    /// decodes it and caches it, returning `Ok(true)`. Otherwise returns `Ok(false)`.
    pub fn cache_packet(&mut self, packet: &RawPacket) -> Result<bool, ProxyError> {
        if packet.id == REGISTRY_DATA_PACKET_ID {
            let reg = RegistryDataPacket::decode(packet)?;
            self.insert(reg);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Processes a client-bound packet: caches `RegistryData`, detects `FinishConfiguration`,
    /// or returns `Passthrough`.
    pub fn process_clientbound_packet(
        &mut self,
        packet: &RawPacket,
    ) -> Result<ConfigAction, ProxyError> {
        match packet.id {
            REGISTRY_DATA_PACKET_ID => {
                let reg = RegistryDataPacket::decode(packet)?;
                let id = reg.registry_id.clone();
                self.insert(reg);
                Ok(ConfigAction::CachedRegistry(id))
            }
            FINISH_CONFIGURATION_PACKET_ID => {
                let _ = FinishConfigurationPacket::decode(packet)?;
                Ok(ConfigAction::TransitionToPlay)
            }
            _ => Ok(ConfigAction::Passthrough(packet.clone())),
        }
    }

    /// Returns the cached packet for a registry identifier.
    pub fn get(&self, registry_id: &str) -> Option<&RegistryDataPacket> {
        self.registries.get(registry_id)
    }

    /// Returns the cached raw NBT bytes for a registry identifier.
    pub fn get_raw(&self, registry_id: &str) -> Option<&Bytes> {
        self.registries.get(registry_id).map(|p| &p.raw_data)
    }

    /// Returns the cached dimension type registry (`minecraft:dimension_type`), if present.
    pub fn dimension_type(&self) -> Option<&RegistryDataPacket> {
        self.get("minecraft:dimension_type")
    }

    /// Returns the cached biome registry (`minecraft:worldgen/biome` or `minecraft:biome`), if present.
    pub fn biomes(&self) -> Option<&RegistryDataPacket> {
        self.get("minecraft:worldgen/biome")
            .or_else(|| self.get("minecraft:biome"))
    }

    /// Returns the cached damage type registry (`minecraft:damage_type`), if present.
    pub fn damage_type(&self) -> Option<&RegistryDataPacket> {
        self.get("minecraft:damage_type")
    }

    /// Checks if a registry identifier is present in the cache.
    pub fn contains(&self, registry_id: &str) -> bool {
        self.registries.contains_key(registry_id)
    }

    /// Number of cached registries.
    pub fn len(&self) -> usize {
        self.registries.len()
    }

    /// Checks if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.registries.is_empty()
    }

    /// Clears all cached registries.
    pub fn clear(&mut self) {
        self.registries.clear();
    }

    /// Serializes all cached registries into `RawPacket`s in deterministic order.
    pub fn serialize_all(&self) -> Vec<RawPacket> {
        self.registries.values().map(|p| p.encode()).collect()
    }

    /// Replays all cached registry packets directly to an asynchronous writer stream
    /// using `write_packet`. Returns the count of replayed packets.
    pub async fn replay_to<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
    ) -> Result<usize, ProxyError> {
        let packets = self.serialize_all();
        let count = packets.len();
        for packet in &packets {
            write_packet(writer, packet).await?;
        }
        Ok(count)
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::network::codec::read_packet;
    use tokio::io::duplex;

    // Helper to generate realistic NBT compound bytes for a mock registry
    fn mock_dimension_type_nbt() -> Vec<u8> {
        // Compound tag header (0x0a), empty name (0x00, 0x00),
        // TAG_String (0x08), name "effects" (len 7), value "minecraft:overworld" (len 19),
        // TAG_Int (0x03), name "height" (len 6), value 384 (0x00000180),
        // TAG_End (0x00)
        vec![
            0x0a, 0x00, 0x00, 0x08, 0x00, 0x07, b'e', b'f', b'f', b'e', b'c', b't', b's', 0x00,
            0x13, b'm', b'i', b'n', b'e', b'c', b'r', b'a', b'f', b't', b':', b'o', b'v', b'e',
            b'r', b'w', b'o', b'r', b'l', b'd', 0x03, 0x00, 0x06, b'h', b'e', b'i', b'g', b'h',
            b't', 0x00, 0x00, 0x01, 0x80, 0x00,
        ]
    }

    fn mock_biome_nbt() -> Vec<u8> {
        // Compound tag header (0x0a), empty name (0x00, 0x00),
        // TAG_Float (0x05), name "temperature" (len 11), value 0.8 (0x3F4CCCCD),
        // TAG_End (0x00)
        vec![
            0x0a, 0x00, 0x00, 0x05, 0x00, 0x0b, b't', b'e', b'm', b'p', b'e', b'r', b'a', b't',
            b'u', b'r', b'e', 0x3f, 0x4c, 0xcc, 0xcd, 0x00,
        ]
    }

    fn mock_damage_type_nbt() -> Vec<u8> {
        // Compound tag header (0x0a), empty name (0x00, 0x00),
        // TAG_String (0x08), name "message_id" (len 10), value "fall" (len 4),
        // TAG_End (0x00)
        vec![
            0x0a, 0x00, 0x00, 0x08, 0x00, 0x0a, b'm', b'e', b's', b's', b'a', b'g', b'e', b'_',
            b'i', b'd', 0x00, 0x04, b'f', b'a', b'l', b'l', 0x00,
        ]
    }

    #[test]
    fn test_registry_data_encode_decode_roundtrip() {
        let nbt = mock_dimension_type_nbt();
        let packet = RegistryDataPacket::new("minecraft:dimension_type", nbt.clone());

        let raw = packet.encode();
        assert_eq!(raw.id, REGISTRY_DATA_PACKET_ID);

        let decoded =
            RegistryDataPacket::decode(&raw).expect("Failed to decode RegistryDataPacket");
        assert_eq!(decoded.registry_id, "minecraft:dimension_type");
        assert_eq!(decoded.raw_data.as_ref(), nbt.as_slice());
        assert_eq!(decoded, packet);
    }

    #[test]
    fn test_session_cache_capture_without_corruption() {
        let mut cache = SessionRegistryCache::new();

        let dim_nbt = mock_dimension_type_nbt();
        let biome_nbt = mock_biome_nbt();
        let dmg_nbt = mock_damage_type_nbt();

        let dim_raw = RegistryDataPacket::new("minecraft:dimension_type", dim_nbt.clone()).encode();
        let biome_raw =
            RegistryDataPacket::new("minecraft:worldgen/biome", biome_nbt.clone()).encode();
        let dmg_raw = RegistryDataPacket::new("minecraft:damage_type", dmg_nbt.clone()).encode();

        assert!(cache.cache_packet(&dim_raw).expect("Failed to cache dim"));
        assert!(cache
            .cache_packet(&biome_raw)
            .expect("Failed to cache biome"));
        assert!(cache
            .cache_packet(&dmg_raw)
            .expect("Failed to cache damage"));

        assert_eq!(cache.len(), 3);
        assert!(!cache.is_empty());

        // Verify dimension type
        assert!(cache.contains("minecraft:dimension_type"));
        let cached_dim = cache.dimension_type().expect("Missing dimension_type");
        assert_eq!(cached_dim.raw_data.as_ref(), dim_nbt.as_slice());
        assert_eq!(
            cache.get_raw("minecraft:dimension_type").unwrap().as_ref(),
            dim_nbt.as_slice()
        );

        // Verify biomes
        assert!(cache.contains("minecraft:worldgen/biome"));
        let cached_biome = cache.biomes().expect("Missing biomes");
        assert_eq!(cached_biome.raw_data.as_ref(), biome_nbt.as_slice());

        // Verify damage type
        assert!(cache.contains("minecraft:damage_type"));
        let cached_dmg = cache.damage_type().expect("Missing damage_type");
        assert_eq!(cached_dmg.raw_data.as_ref(), dmg_nbt.as_slice());
    }

    #[test]
    fn test_finish_configuration_packet() {
        let finish = FinishConfigurationPacket::new();
        let raw = finish.encode();
        assert_eq!(raw.id, FINISH_CONFIGURATION_PACKET_ID);
        assert!(raw.payload.is_empty());

        let decoded =
            FinishConfigurationPacket::decode(&raw).expect("Failed to decode FinishConfiguration");
        assert_eq!(decoded.next_state(), ConnectionState::Play);
    }

    #[test]
    fn test_process_clientbound_packet_actions() {
        let mut cache = SessionRegistryCache::new();

        // 1. Registry Data packet
        let dim_raw =
            RegistryDataPacket::new("minecraft:dimension_type", mock_dimension_type_nbt()).encode();
        let action = cache
            .process_clientbound_packet(&dim_raw)
            .expect("Process failed");
        assert_eq!(
            action,
            ConfigAction::CachedRegistry("minecraft:dimension_type".into())
        );
        assert!(cache.contains("minecraft:dimension_type"));

        // 2. Finish Configuration packet
        let finish_raw = FinishConfigurationPacket::new().encode();
        let action = cache
            .process_clientbound_packet(&finish_raw)
            .expect("Process failed");
        assert_eq!(action, ConfigAction::TransitionToPlay);

        // 3. Passthrough packet (e.g. KeepAlive 0x03)
        let keep_alive = RawPacket::new(0x03, Bytes::from_static(&[0x01, 0x02, 0x03]));
        let action = cache
            .process_clientbound_packet(&keep_alive)
            .expect("Process failed");
        assert_eq!(action, ConfigAction::Passthrough(keep_alive));
    }

    #[tokio::test]
    async fn test_cache_replay_to_stream() {
        let mut cache = SessionRegistryCache::new();

        let dim_nbt = mock_dimension_type_nbt();
        let biome_nbt = mock_biome_nbt();
        let dmg_nbt = mock_damage_type_nbt();

        cache.insert_raw("minecraft:dimension_type", dim_nbt.clone());
        cache.insert_raw("minecraft:worldgen/biome", biome_nbt.clone());
        cache.insert_raw("minecraft:damage_type", dmg_nbt.clone());

        let (mut client_writer, mut client_reader) = duplex(65536);

        let replayed_count = cache
            .replay_to(&mut client_writer)
            .await
            .expect("Replay failed");
        assert_eq!(replayed_count, 3);

        // Read all 3 replayed packets from the client stream
        let mut replayed_packets = Vec::new();
        for _ in 0..3 {
            let packet = read_packet(&mut client_reader, 1024 * 1024)
                .await
                .expect("Read packet failed");
            replayed_packets.push(packet);
        }

        // Verify each replayed packet matches its original cached data
        for raw in &replayed_packets {
            let reg = RegistryDataPacket::decode(raw).expect("Decode replayed packet failed");
            match reg.registry_id.as_str() {
                "minecraft:damage_type" => assert_eq!(reg.raw_data.as_ref(), dmg_nbt.as_slice()),
                "minecraft:dimension_type" => assert_eq!(reg.raw_data.as_ref(), dim_nbt.as_slice()),
                "minecraft:worldgen/biome" => {
                    assert_eq!(reg.raw_data.as_ref(), biome_nbt.as_slice())
                }
                other => panic!("Unexpected registry ID: {other}"),
            }
        }
    }

    #[test]
    fn test_invalid_packet_id_rejection() {
        let bad_raw = RawPacket::new(0x99, Bytes::new());
        assert!(RegistryDataPacket::decode(&bad_raw).is_err());
        assert!(FinishConfigurationPacket::decode(&bad_raw).is_err());

        let mut cache = SessionRegistryCache::new();
        let cached = cache
            .cache_packet(&bad_raw)
            .expect("Should not error for non-registry packet");
        assert!(!cached);
    }

    #[test]
    fn test_registry_entries_roundtrip() {
        let entry1 = RegistryEntry::new("minecraft:overworld", Some(vec![0x01, 0x02, 0x03]));
        let packet = RegistryDataPacket::from_entries(
            "minecraft:dimension_type",
            std::slice::from_ref(&entry1),
        );

        let decoded_entries = packet
            .try_decode_entries()
            .expect("Failed to decode entries");
        assert_eq!(decoded_entries.len(), 1);
        assert_eq!(decoded_entries[0].entry_id, "minecraft:overworld");
        assert_eq!(
            decoded_entries[0].data.as_ref().unwrap().as_ref(),
            &[0x01, 0x02, 0x03]
        );
    }
}
