use std::{
    collections::VecDeque,
    env,
    error::Error,
    io::{self, Write},
    sync::mpsc::{self, SyncSender},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use iroh::{Endpoint, EndpointAddr, endpoint::presets};
use wasapi::*;

#[path = "../src/media.rs"]
mod media;

const ALPN: &[u8] = b"/seafriend/audio/1";
const SAMPLE_RATE: usize = 48_000;
const CHANNELS: usize = 2;
const CHUNK_FRAMES: usize = 1024;
const RUN_FOR: Duration = Duration::from_secs(10);
type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

fn capture_loop(sender: SyncSender<Vec<u8>>, process_id: u32) -> Result<()> {
    initialize_mta()
        .ok()
        .map_err(|error| format!("COM initialization failed: {error:?}"))?;
    let format = WaveFormat::new(32, 32, SampleType::Float, SAMPLE_RATE, CHANNELS, None);
    let block_align = format.get_blockalign() as usize;
    let mut client = AudioClient::new_application_loopback_client(process_id, true)?;
    client.initialize_client(
        &format,
        Direction::Capture,
        StreamMode::EventsShared {
            autoconvert: true,
            buffer_duration_hns: 0,
        },
        None,
    )?;
    let event = client.set_get_eventhandle()?;
    let capture = client.get_audiocaptureclient()?;
    let started = Instant::now();
    let mut queued = VecDeque::new();
    client.start_stream()?;
    while started.elapsed() < RUN_FOR {
        while queued.len() >= block_align * CHUNK_FRAMES {
            let mut chunk = vec![0; block_align * CHUNK_FRAMES];
            for byte in &mut chunk {
                *byte = queued.pop_front().expect("audio chunk has enough bytes");
            }
            sender.send(chunk)?;
        }
        let frame_count = capture.get_next_packet_size()?.unwrap_or(0);
        if frame_count > 0 {
            let additional = (frame_count as usize * block_align)
                .saturating_sub(queued.capacity().saturating_sub(queued.len()));
            queued.reserve(additional);
            capture.read_from_device_to_deque(&mut queued)?;
        }
        event.wait_for_event(1000)?;
    }
    client.stop_stream()?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    let mode = args
        .next()
        .ok_or("usage: audio_stream listen|connect [addr] [pid]")?;
    let endpoint = Endpoint::builder(presets::N0)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await?;
    let address = URL_SAFE_NO_PAD.encode(postcard::to_stdvec(&endpoint.addr())?);
    println!("endpoint_addr_b64={address}");
    io::stdout().flush()?;
    match mode.as_str() {
        "listen" => receive_audio(&endpoint).await?,
        "connect" => {
            let remote: EndpointAddr = postcard::from_bytes(
                &URL_SAFE_NO_PAD.decode(args.next().ok_or("connect requires addr")?)?,
            )?;
            let process_id: u32 = args.next().ok_or("connect requires process id")?.parse()?;
            send_audio(&endpoint, remote, process_id).await?;
        }
        _ => return Err("mode must be listen or connect".into()),
    }
    endpoint.close().await;
    Ok(())
}

async fn receive_audio(endpoint: &Endpoint) -> Result<()> {
    let incoming = endpoint.accept().await.ok_or("endpoint closed")?;
    let connection = incoming.await?;
    let (mut send, mut receive) = connection.accept_bi().await?;
    let mut tracker = media::SequenceTracker::default();
    let mut packets = 0u64;
    let (playback_sender, playback_receiver) = mpsc::sync_channel(4);
    let playback_thread = thread::spawn(move || playback_loop(playback_receiver));
    let deadline = Instant::now() + RUN_FOR + Duration::from_secs(5);
    while Instant::now() < deadline {
        let mut header = [0u8; media::MEDIA_FRAME_HEADER_BYTES];
        if tokio::time::timeout(Duration::from_secs(2), receive.read_exact(&mut header))
            .await
            .is_err()
        {
            break;
        }
        let length = u32::from_le_bytes(header) as usize;
        if length > media::MAX_MEDIA_PACKET_BYTES {
            return Err("audio packet exceeds maximum size".into());
        }
        let mut payload = vec![0; length];
        receive.read_exact(&mut payload).await?;
        let mut framed = header.to_vec();
        framed.extend_from_slice(&payload);
        let packet =
            media::MediaPacket::decode_framed(&framed).map_err(|error| error.to_owned())?;
        if packet.kind != media::MediaKind::Audio || packet.codec != media::MediaCodec::PcmF32Le {
            return Err("received non-audio packet on audio stream".into());
        }
        tracker.accept(&packet).map_err(|error| error.to_owned())?;
        if playback_sender.send(packet.payload).is_err() {
            break;
        }
        packets += 1;
    }
    drop(playback_sender);
    playback_thread
        .join()
        .map_err(|_| "audio playback thread panicked")??;
    send.write_all(&packets.to_le_bytes()).await?;
    send.finish()?;
    println!("received {packets} audio packets");
    Ok(())
}

fn playback_loop(receiver: mpsc::Receiver<Vec<u8>>) -> Result<()> {
    initialize_mta()
        .ok()
        .map_err(|error| format!("COM initialization failed: {error:?}"))?;
    let enumerator = DeviceEnumerator::new()?;
    let device = enumerator.get_default_device(Direction::Render)?;
    let mut client = device.get_iaudioclient()?;
    let format = WaveFormat::new(32, 32, SampleType::Float, SAMPLE_RATE, CHANNELS, None);
    let (_, minimum_period) = client.get_device_period()?;
    client.initialize_client(
        &format,
        Direction::Render,
        StreamMode::EventsShared {
            autoconvert: true,
            buffer_duration_hns: minimum_period,
        },
        None,
    )?;
    let event = client.set_get_eventhandle()?;
    let render = client.get_audiorenderclient()?;
    let block_align = format.get_blockalign() as usize;
    let mut queued = VecDeque::with_capacity(block_align * SAMPLE_RATE / 2);
    client.start_stream()?;
    loop {
        let available = client.get_available_space_in_frames()? as usize;
        let required = available * block_align;
        while queued.len() < required {
            match receiver.recv_timeout(Duration::from_millis(20)) {
                Ok(chunk) => queued.extend(chunk),
                Err(mpsc::RecvTimeoutError::Timeout) => queued.resize(required, 0),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    client.stop_stream()?;
                    return Ok(());
                }
            }
        }
        render.write_to_device_from_deque(available, &mut queued, None)?;
        event.wait_for_event(1000)?;
    }
}

async fn send_audio(endpoint: &Endpoint, remote: EndpointAddr, process_id: u32) -> Result<()> {
    let connection = endpoint.connect(remote, ALPN).await?;
    let (mut send, mut receive) = connection.open_bi().await?;
    let (sender, receiver) = mpsc::sync_channel(2);
    let capture_thread = thread::spawn(move || capture_loop(sender, process_id));
    let mut sequence = 0u64;
    while let Ok(payload) = receiver.recv() {
        let packet = media::MediaPacket::audio_pcm_f32(
            1,
            sequence,
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_micros() as u64,
            SAMPLE_RATE as u32,
            CHANNELS as u32,
            payload,
        );
        send.write_all(&packet.encode_framed()?).await?;
        sequence += 1;
    }
    send.finish()?;
    let acknowledgement = receive.read_to_end(8).await?;
    let received = acknowledgement
        .get(..8)
        .and_then(|bytes| bytes.try_into().ok().map(u64::from_le_bytes))
        .unwrap_or(0);
    capture_thread
        .join()
        .map_err(|_| "audio capture thread panicked")??;
    println!("sent {sequence} audio packets; receiver confirmed {received}");
    Ok(())
}
