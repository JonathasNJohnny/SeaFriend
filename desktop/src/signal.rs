use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::{SinkExt, StreamExt};
use iroh::{Endpoint, EndpointAddr, endpoint::presets};
use serde_json::json;
use std::collections::HashSet;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc::{Receiver, Sender, SyncSender},
};
use tokio::sync::broadcast;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::media::{
    CapturedFrame, MAX_MEDIA_FRAME_BYTES, MAX_MEDIA_PACKET_BYTES, MediaJitterBuffer, MediaKind,
    MediaPacket,
};

const MEDIA_ALPN: &[u8] = b"/seafriend/media/1";

#[allow(clippy::too_many_arguments)]
pub fn start(
    base_url: String,
    room_id: String,
    participant_id: String,
    events: Sender<String>,
    media_frames: Receiver<CapturedFrame>,
    display_frames: SyncSender<CapturedFrame>,
    audio_frames: Receiver<Vec<u8>>,
    audio_playback: SyncSender<Vec<u8>>,
) -> Arc<AtomicBool> {
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    std::thread::spawn(move || {
        let Ok(runtime) = tokio::runtime::Runtime::new() else {
            let _ = events.send("failed to start signaling runtime".to_owned());
            return;
        };
        runtime.block_on(async move {
            let (frame_broadcast, _) = broadcast::channel(2);
            let frame_sender = frame_broadcast.clone();
            let frame_stop = thread_stop.clone();
            std::thread::spawn(move || {
                while !frame_stop.load(Ordering::Relaxed) {
                    match media_frames.recv_timeout(std::time::Duration::from_millis(100)) {
                        Ok(frame) => {
                            let _ = frame_sender.send(frame);
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            });
            let (audio_broadcast, _) = broadcast::channel(4);
            let audio_sender = audio_broadcast.clone();
            let audio_stop = thread_stop.clone();
            std::thread::spawn(move || {
                while !audio_stop.load(Ordering::Relaxed) {
                    match audio_frames.recv_timeout(std::time::Duration::from_millis(100)) {
                        Ok(chunk) => {
                            let _ = audio_sender.send(chunk);
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            });
            let endpoint = match Endpoint::builder(presets::N0)
                .alpns(vec![MEDIA_ALPN.to_vec()])
                .bind()
                .await
            {
                Ok(endpoint) => endpoint,
                Err(error) => {
                    let _ = events.send(format!("failed to start P2P endpoint: {error}"));
                    return;
                }
            };
            let connected_peers = Arc::new(Mutex::new(HashSet::<String>::new()));
            let encoded_endpoint = match postcard::to_stdvec(&endpoint.addr()) {
                Ok(bytes) => URL_SAFE_NO_PAD.encode(bytes),
                Err(error) => {
                    let _ = events.send(format!("failed to serialize P2P endpoint: {error}"));
                    return;
                }
            };
            let websocket_url = base_url
                .trim_end_matches('/')
                .replacen("https://", "wss://", 1)
                .replacen("http://", "ws://", 1);
            let url = format!(
                "{websocket_url}/rooms/{room_id}/signal?participant_id={participant_id}"
            );
            let accept_endpoint = endpoint.clone();
            let accept_events = events.clone();
            let accept_frames = frame_broadcast.clone();
            let accept_audio_playback = audio_playback.clone();
            tokio::spawn(async move {
                while let Some(incoming) = accept_endpoint.accept().await {
                    match incoming.await {
                        Ok(connection) => {
                            let _ = accept_events.send("P2P connection received".to_owned());
                            let receiver_frames = accept_frames.clone();
                            let receiver_events = accept_events.clone();
                            let receiver_display = display_frames.clone();
                            let receiver_audio_playback = accept_audio_playback.clone();
                            tokio::spawn(async move {
                                run_media_receiver(
                                    connection,
                                    receiver_frames,
                                    receiver_events,
                                    receiver_display,
                                    receiver_audio_playback,
                                )
                                .await;
                            });
                        }
                        Err(error) => {
                            let _ = accept_events.send(format!("P2P connection error: {error}"));
                        }
                    }
                }
            });

            let mut reconnect_delay = std::time::Duration::from_secs(1);
            while !thread_stop.load(Ordering::Relaxed) {
                let Ok((mut socket, _)) = connect_async(url.clone()).await else {
                    let _ = events.send(format!(
                        "signaling unavailable; retrying in {}s",
                        reconnect_delay.as_secs()
                    ));
                    tokio::time::sleep(reconnect_delay).await;
                    reconnect_delay = (reconnect_delay * 2).min(std::time::Duration::from_secs(30));
                    continue;
                };
                reconnect_delay = std::time::Duration::from_secs(1);
                let _ = events.send("signaling connected".to_owned());
                let _ = socket
                    .send(Message::Text(
                        json!({"kind": "signal", "data": {"type": "endpoint", "address": encoded_endpoint}})
                            .to_string()
                            .into(),
                    ))
                    .await;

                let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(15));
                loop {
                    tokio::select! {
                        _ = heartbeat.tick() => {
                            if thread_stop.load(Ordering::Relaxed) {
                                break;
                            }
                            if socket.send(Message::Text(json!({"kind": "heartbeat"}).to_string().into())).await.is_err() {
                                break;
                            }
                        }
                        message = socket.next() => {
                            let Some(Ok(Message::Text(text))) = message else { break; };
                            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text)
                                && let Some(kind) = value.get("kind").and_then(serde_json::Value::as_str)
                            {
                                let _ = events.send(format!("signaling: {kind}"));
                                if kind == "signal"
                                    && let Some(data) = value.get("data")
                                    && data.get("type").and_then(serde_json::Value::as_str) == Some("endpoint")
                                    && let Some(address) = data.get("address").and_then(serde_json::Value::as_str)
                                    && let Ok(bytes) = URL_SAFE_NO_PAD.decode(address)
                                    && let Ok(remote) = postcard::from_bytes::<EndpointAddr>(&bytes)
                                {
                                    let peer_key = address.to_owned();
                                    let already_connected = connected_peers
                                        .lock()
                                        .map(|mut peers| !peers.insert(peer_key.clone()))
                                        .unwrap_or(true);
                                    if already_connected {
                                        continue;
                                    }
                                    let connect_endpoint = endpoint.clone();
                                    let connect_events = events.clone();
                                    let sender_frames = frame_broadcast.clone();
                                    let sender_audio = audio_broadcast.clone();
                                    let connected_peers = connected_peers.clone();
                                    tokio::spawn(async move {
                                        match connect_endpoint.connect(remote, MEDIA_ALPN).await {
                                            Ok(connection) => {
                                                let _ = connect_events.send("P2P connection established".to_owned());
                                                run_media_sender(
                                                    connection,
                                                    sender_frames,
                                                    sender_audio,
                                                    connect_events,
                                                )
                                                .await;
                                            }
                                            Err(error) => {
                                                let _ = connect_events.send(format!("P2P connection failed: {error}"));
                                            }
                                        }
                                        if let Ok(mut peers) = connected_peers.lock() {
                                            peers.remove(&peer_key);
                                        }
                                    });
                                }
                            }
                        }
                    }
                }
                if !thread_stop.load(Ordering::Relaxed) {
                    let _ = events.send("signaling disconnected; reconnecting".to_owned());
                    tokio::time::sleep(reconnect_delay).await;
                }
            }
            let _ = events.send("signaling disconnected".to_owned());
            endpoint.close().await;
        });
    });
    stop
}

async fn run_media_sender(
    connection: iroh::endpoint::Connection,
    frames: broadcast::Sender<CapturedFrame>,
    audio: broadcast::Sender<Vec<u8>>,
    events: Sender<String>,
) {
    let Ok((mut send, _)) = connection.open_bi().await else {
        let _ = events.send("failed to open media stream".to_owned());
        return;
    };
    let mut received_frames = frames.subscribe();
    let mut received_audio = audio.subscribe();
    let mut sequence = 0;
    loop {
        tokio::select! {
            frame = received_frames.recv() => {
                let Ok(frame) = frame else { continue; };
                let packet = match MediaPacket::video_deflate(
                    1,
                    sequence,
                    monotonic_micros(),
                    frame.width,
                    frame.height,
                    &frame.pixels,
                ) {
                    Ok(packet) => packet,
                    Err(error) => {
                        let _ = events.send(format!("video compression failed: {error}"));
                        continue;
                    }
                };
                let Ok(encoded) = packet.encode_framed() else { break; };
                if encoded.len() <= MAX_MEDIA_FRAME_BYTES && send.write_all(&encoded).await.is_err() { break; }
                sequence += 1;
            }
            audio_chunk = received_audio.recv() => {
                let Ok(audio_chunk) = audio_chunk else { continue; };
                let packet = MediaPacket::audio_pcm_f32(2, sequence, monotonic_micros(), crate::audio::SAMPLE_RATE as u32, crate::audio::CHANNELS as u32, audio_chunk);
                let Ok(encoded) = packet.encode_framed() else { break; };
                if encoded.len() <= MAX_MEDIA_FRAME_BYTES && send.write_all(&encoded).await.is_err() { break; }
                sequence += 1;
            }
        }
    }
    let _ = events.send(format!("media sender stopped after {sequence} frames"));
}

async fn run_media_receiver(
    connection: iroh::endpoint::Connection,
    _frames: broadcast::Sender<CapturedFrame>,
    events: Sender<String>,
    display_frames: SyncSender<CapturedFrame>,
    audio_playback: SyncSender<Vec<u8>>,
) {
    let Ok((_send, mut receive)) = connection.accept_bi().await else {
        let _ = events.send("failed to accept media stream".to_owned());
        return;
    };
    let mut frames = 0u64;
    let mut sequence_tracker = crate::media::SequenceTracker::default();
    let mut jitter_buffer = MediaJitterBuffer::new(120);
    let mut latest_timestamp = 0u64;
    loop {
        let mut header = [0u8; 4];
        if receive.read_exact(&mut header).await.is_err() {
            break;
        }
        let length = u32::from_le_bytes(header) as usize;
        if length > MAX_MEDIA_PACKET_BYTES {
            break;
        }
        let mut payload = vec![0u8; length];
        if receive.read_exact(&mut payload).await.is_err() {
            break;
        }
        let mut framed = header.to_vec();
        framed.extend_from_slice(&payload);
        let Ok(packet) = MediaPacket::decode_framed(&framed) else {
            break;
        };
        if sequence_tracker.accept(&packet).is_err() {
            break;
        }
        latest_timestamp = latest_timestamp.max(packet.timestamp_micros);
        jitter_buffer.push(packet);
        for ready_packet in jitter_buffer.pop_ready(latest_timestamp, 50_000) {
            if ready_packet.kind == MediaKind::Video {
                let Ok(pixels) = ready_packet.decode_video_pixels() else {
                    continue;
                };
                let _ = display_frames.try_send(CapturedFrame {
                    width: ready_packet.width,
                    height: ready_packet.height,
                    pixels,
                });
            } else if ready_packet.kind == MediaKind::Audio {
                let _ = audio_playback.try_send(ready_packet.payload);
            }
        }
        frames += 1;
    }
    let _ = events.send(format!("media receiver stopped after {frames} frames"));
}

fn monotonic_micros() -> u64 {
    use std::sync::OnceLock;
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_micros() as u64
}
