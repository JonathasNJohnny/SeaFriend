use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;

pub const MEDIA_PROTOCOL_VERSION: u8 = 1;
pub const MAX_MEDIA_PACKET_BYTES: usize = 8 * 1024 * 1024;
pub const MEDIA_FRAME_HEADER_BYTES: usize = 4;
pub const MAX_MEDIA_FRAME_BYTES: usize = MEDIA_FRAME_HEADER_BYTES + MAX_MEDIA_PACKET_BYTES;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct CapturedFrame {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum MediaKind {
    Video,
    Audio,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum MediaCodec {
    RawRgba8,
    DeflateRgba8,
    PcmF32Le,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MediaPacket {
    pub version: u8,
    pub kind: MediaKind,
    pub codec: MediaCodec,
    pub stream_id: u32,
    pub sequence: u64,
    pub timestamp_micros: u64,
    pub width: u32,
    pub height: u32,
    pub payload: Vec<u8>,
}

impl MediaPacket {
    #[allow(dead_code)]
    pub fn video(
        stream_id: u32,
        sequence: u64,
        timestamp_micros: u64,
        width: u32,
        height: u32,
        payload: Vec<u8>,
    ) -> Self {
        Self {
            version: MEDIA_PROTOCOL_VERSION,
            kind: MediaKind::Video,
            codec: MediaCodec::RawRgba8,
            stream_id,
            sequence,
            timestamp_micros,
            width,
            height,
            payload,
        }
    }

    #[allow(dead_code)]
    pub fn audio_pcm_f32(
        stream_id: u32,
        sequence: u64,
        timestamp_micros: u64,
        sample_rate: u32,
        channels: u32,
        payload: Vec<u8>,
    ) -> Self {
        Self {
            version: MEDIA_PROTOCOL_VERSION,
            kind: MediaKind::Audio,
            codec: MediaCodec::PcmF32Le,
            stream_id,
            sequence,
            timestamp_micros,
            width: sample_rate,
            height: channels,
            payload,
        }
    }

    #[allow(dead_code)]
    pub fn video_deflate(
        stream_id: u32,
        sequence: u64,
        timestamp_micros: u64,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> Result<Self, &'static str> {
        let expected = (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(4);
        if width == 0
            || height == 0
            || expected > MAX_MEDIA_PACKET_BYTES
            || pixels.len() != expected
        {
            return Err("video pixels do not match dimensions");
        }
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut encoder, pixels).map_err(|_| "video compression failed")?;
        let payload = encoder.finish().map_err(|_| "video compression failed")?;
        Ok(Self {
            version: MEDIA_PROTOCOL_VERSION,
            kind: MediaKind::Video,
            codec: MediaCodec::DeflateRgba8,
            stream_id,
            sequence,
            timestamp_micros,
            width,
            height,
            payload,
        })
    }

    pub fn decode_video_pixels(&self) -> Result<Vec<u8>, &'static str> {
        if self.kind != MediaKind::Video {
            return Err("media packet is not video");
        }
        let expected = (self.width as usize)
            .saturating_mul(self.height as usize)
            .saturating_mul(4);
        if expected > MAX_MEDIA_PACKET_BYTES {
            return Err("video frame exceeds maximum size");
        }
        match self.codec {
            MediaCodec::RawRgba8 => {
                if self.payload.len() != expected {
                    return Err("video packet payload does not match dimensions");
                }
                Ok(self.payload.clone())
            }
            MediaCodec::DeflateRgba8 => {
                let decoder = flate2::read::ZlibDecoder::new(self.payload.as_slice());
                let mut pixels = Vec::with_capacity(expected.min(MAX_MEDIA_PACKET_BYTES));
                decoder
                    .take((expected as u64).saturating_add(1))
                    .read_to_end(&mut pixels)
                    .map_err(|_| "video decompression failed")?;
                if pixels.len() != expected {
                    return Err("decompressed video payload does not match dimensions");
                }
                Ok(pixels)
            }
            MediaCodec::PcmF32Le => Err("video packet has incompatible codec"),
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>, postcard::Error> {
        postcard::to_stdvec(self)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, postcard::Error> {
        postcard::from_bytes(bytes)
    }

    pub fn encode_framed(&self) -> Result<Vec<u8>, postcard::Error> {
        let encoded = self.encode()?;
        let length = u32::try_from(encoded.len()).unwrap_or(u32::MAX);
        let mut framed = Vec::with_capacity(MEDIA_FRAME_HEADER_BYTES + encoded.len());
        framed.extend_from_slice(&length.to_le_bytes());
        framed.extend_from_slice(&encoded);
        Ok(framed)
    }

    pub fn decode_framed(frame: &[u8]) -> Result<Self, &'static str> {
        if frame.len() > MAX_MEDIA_FRAME_BYTES {
            return Err("media frame exceeds maximum size");
        }
        if frame.len() < MEDIA_FRAME_HEADER_BYTES {
            return Err("media frame is missing its length header");
        }
        let declared =
            u32::from_le_bytes(frame[..MEDIA_FRAME_HEADER_BYTES].try_into().unwrap()) as usize;
        if declared > MAX_MEDIA_PACKET_BYTES {
            return Err("media frame exceeds maximum size");
        }
        if declared != frame.len() - MEDIA_FRAME_HEADER_BYTES {
            return Err("media frame length does not match payload");
        }
        let packet = Self::decode(&frame[MEDIA_FRAME_HEADER_BYTES..])
            .map_err(|_| "media frame payload is invalid")?;
        packet.validate()?;
        Ok(packet)
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.version != MEDIA_PROTOCOL_VERSION {
            return Err("unsupported media protocol version");
        }
        if self.payload.len() > MAX_MEDIA_PACKET_BYTES {
            return Err("media packet exceeds maximum size");
        }
        if self.stream_id == 0 {
            return Err("media packet has invalid stream id");
        }
        match (self.kind, self.codec) {
            (MediaKind::Video, MediaCodec::RawRgba8 | MediaCodec::DeflateRgba8)
                if self.width == 0 || self.height == 0 =>
            {
                return Err("video packet has invalid dimensions");
            }
            (MediaKind::Video, MediaCodec::RawRgba8 | MediaCodec::DeflateRgba8) => {
                self.decode_video_pixels()?;
            }
            (MediaKind::Video, MediaCodec::PcmF32Le) => {
                return Err("video packet has incompatible codec");
            }
            (MediaKind::Audio, MediaCodec::RawRgba8) => {
                return Err("audio packet has incompatible codec");
            }
            (MediaKind::Audio, MediaCodec::DeflateRgba8) => {
                return Err("audio packet has incompatible codec");
            }
            (MediaKind::Audio, MediaCodec::PcmF32Le)
                if self.width == 0
                    || self.width > 192_000
                    || self.height == 0
                    || self.height > 8 =>
            {
                return Err("audio packet has invalid format");
            }
            (MediaKind::Audio, MediaCodec::PcmF32Le) => {
                let frame_bytes = (self.height as usize).saturating_mul(4);
                if frame_bytes == 0 || !self.payload.len().is_multiple_of(frame_bytes) {
                    return Err("audio packet payload is not aligned to PCM frames");
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct SequenceTracker {
    last_by_stream: HashMap<u32, u64>,
}

#[allow(dead_code)]
#[derive(Debug)]
pub struct MediaJitterBuffer {
    packets: Vec<MediaPacket>,
    capacity: usize,
}

#[allow(dead_code)]
impl MediaJitterBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            packets: Vec::with_capacity(capacity),
            capacity: capacity.max(1),
        }
    }

    pub fn push(&mut self, packet: MediaPacket) {
        if self.packets.len() >= self.capacity {
            self.packets.remove(0);
        }
        self.packets.push(packet);
        self.packets
            .sort_unstable_by_key(|packet| packet.timestamp_micros);
    }

    pub fn pop_ready(&mut self, playback_timestamp: u64, latency_micros: u64) -> Vec<MediaPacket> {
        let cutoff = playback_timestamp.saturating_sub(latency_micros);
        let split = self
            .packets
            .partition_point(|packet| packet.timestamp_micros <= cutoff);
        self.packets.drain(..split).collect()
    }

    pub fn len(&self) -> usize {
        self.packets.len()
    }
}

impl SequenceTracker {
    pub fn accept(&mut self, packet: &MediaPacket) -> Result<(), &'static str> {
        if let Some(last_sequence) = self.last_by_stream.get(&packet.stream_id).copied()
            && packet.sequence <= last_sequence
        {
            return Err("media sequence moved backwards");
        }
        self.last_by_stream
            .insert(packet.stream_id, packet.sequence);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_packet_round_trips() {
        let packet = MediaPacket::video(7, 42, 123_456, 2, 1, vec![1, 2, 3, 4, 5, 6, 7, 8]);
        let encoded = packet.encode().unwrap();
        let decoded = MediaPacket::decode(&encoded).unwrap();
        assert_eq!(decoded, packet);
        decoded.validate().unwrap();
    }

    #[test]
    fn rejects_invalid_video_dimensions() {
        let packet = MediaPacket::video(1, 1, 0, 0, 180, vec![]);
        assert_eq!(
            packet.validate(),
            Err("video packet has invalid dimensions")
        );
    }

    #[test]
    fn audio_packet_round_trips_with_format() {
        let packet = MediaPacket::audio_pcm_f32(2, 4, 999, 48_000, 2, vec![0; 16]);
        let encoded = packet.encode().unwrap();
        let decoded = MediaPacket::decode(&encoded).unwrap();
        assert_eq!(decoded, packet);
        assert_eq!(decoded.codec, MediaCodec::PcmF32Le);
        decoded.validate().unwrap();
    }

    #[test]
    fn compressed_video_round_trips_and_validates_pixels() {
        let pixels = vec![7; 64];
        let packet = MediaPacket::video_deflate(1, 2, 3, 4, 4, &pixels).unwrap();
        assert!(packet.payload.len() < pixels.len());
        packet.validate().unwrap();
        assert_eq!(packet.decode_video_pixels().unwrap(), pixels);
        let encoded = packet.encode().unwrap();
        let decoded = MediaPacket::decode(&encoded).unwrap();
        decoded.validate().unwrap();
        assert_eq!(decoded.decode_video_pixels().unwrap(), pixels);
    }

    #[test]
    fn rejects_corrupt_compressed_video() {
        let mut packet = MediaPacket::video_deflate(1, 2, 3, 2, 2, &[1; 16]).unwrap();
        packet.payload[0] ^= 0xff;
        assert_eq!(packet.validate(), Err("video decompression failed"));
    }

    #[test]
    fn rejects_compressed_video_with_oversized_output_dimensions() {
        let packet = MediaPacket {
            version: MEDIA_PROTOCOL_VERSION,
            kind: MediaKind::Video,
            codec: MediaCodec::DeflateRgba8,
            stream_id: 1,
            sequence: 2,
            timestamp_micros: 3,
            width: 100_000,
            height: 100,
            payload: vec![0; 8],
        };
        assert_eq!(packet.validate(), Err("video frame exceeds maximum size"));
    }

    #[test]
    fn rejects_incompatible_codec() {
        let mut packet = MediaPacket::video(1, 1, 0, 320, 180, vec![]);
        packet.codec = MediaCodec::PcmF32Le;
        assert_eq!(
            packet.validate(),
            Err("video packet has incompatible codec")
        );
    }

    #[test]
    fn rejects_misaligned_audio_payload() {
        let packet = MediaPacket::audio_pcm_f32(2, 4, 999, 48_000, 2, vec![0; 6]);
        assert_eq!(
            packet.validate(),
            Err("audio packet payload is not aligned to PCM frames")
        );
    }

    #[test]
    fn rejects_unreasonable_audio_format() {
        let packet = MediaPacket::audio_pcm_f32(2, 4, 999, 384_000, 2, vec![0; 8]);
        assert_eq!(packet.validate(), Err("audio packet has invalid format"));
    }

    #[test]
    fn framed_packet_round_trips_and_checks_length() {
        let packet = MediaPacket::video(1, 3, 10, 1, 1, vec![1, 2, 3, 4]);
        let frame = packet.encode_framed().unwrap();
        assert_eq!(MediaPacket::decode_framed(&frame).unwrap(), packet);

        let mut truncated = frame;
        truncated.pop();
        assert_eq!(
            MediaPacket::decode_framed(&truncated),
            Err("media frame length does not match payload")
        );
    }

    #[test]
    fn sequence_tracker_rejects_duplicates_and_allows_new_streams() {
        let mut tracker = SequenceTracker::default();
        let first = MediaPacket::video(1, 4, 0, 1, 1, vec![0; 4]);
        let duplicate = MediaPacket::video(1, 4, 1, 1, 1, vec![0; 4]);
        let next_stream = MediaPacket::video(2, 0, 2, 1, 1, vec![0; 4]);
        tracker.accept(&first).unwrap();
        assert_eq!(
            tracker.accept(&duplicate),
            Err("media sequence moved backwards")
        );
        tracker.accept(&next_stream).unwrap();
        assert_eq!(
            tracker.accept(&first),
            Err("media sequence moved backwards")
        );
    }

    #[test]
    fn jitter_buffer_orders_packets_and_applies_latency_window() {
        let mut buffer = MediaJitterBuffer::new(4);
        buffer.push(MediaPacket::video(1, 2, 120, 1, 1, vec![0; 4]));
        buffer.push(MediaPacket::video(1, 1, 100, 1, 1, vec![0; 4]));
        buffer.push(MediaPacket::video(1, 3, 140, 1, 1, vec![0; 4]));

        let ready = buffer.pop_ready(140, 30);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].timestamp_micros, 100);
        assert_eq!(buffer.len(), 2);
    }
}
