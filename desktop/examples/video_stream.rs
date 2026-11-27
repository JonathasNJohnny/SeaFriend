use std::{
    env,
    error::Error,
    io::{self, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use iroh::{Endpoint, EndpointAddr, endpoint::presets};
use tokio::sync::mpsc;
use windows_capture::{
    capture::{Context, GraphicsCaptureApiHandler},
    frame::Frame,
    graphics_capture_api::InternalCaptureControl,
    graphics_capture_picker::GraphicsCapturePicker,
    settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
        MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
    },
};

#[path = "../src/media.rs"]
mod media;

const ALPN: &[u8] = b"/seafriend/video/1";
const RUN_FOR: Duration = Duration::from_secs(10);

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

struct CapturedFrame {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

struct CaptureHandler {
    sender: mpsc::Sender<CapturedFrame>,
    stop: Arc<AtomicBool>,
    last_frame: Instant,
}

impl GraphicsCaptureApiHandler for CaptureHandler {
    type Flags = (mpsc::Sender<CapturedFrame>, Arc<AtomicBool>);
    type Error = Box<dyn Error + Send + Sync>;

    fn new(ctx: Context<Self::Flags>) -> Result<Self> {
        Ok(Self {
            sender: ctx.flags.0,
            stop: ctx.flags.1,
            last_frame: Instant::now() - Duration::from_secs(1),
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        capture_control: InternalCaptureControl,
    ) -> Result<()> {
        if self.stop.load(Ordering::Relaxed) {
            capture_control.stop();
            return Ok(());
        }
        if self.last_frame.elapsed() < Duration::from_millis(100) {
            return Ok(());
        }
        let buffer = frame.buffer()?;
        let mut pixels = Vec::new();
        let bytes = buffer.as_nopadding_buffer(&mut pixels).to_vec();
        self.sender.blocking_send(CapturedFrame {
            width: buffer.width(),
            height: buffer.height(),
            pixels: bytes,
        })?;
        self.last_frame = Instant::now();
        Ok(())
    }

    fn on_closed(&mut self) -> Result<()> {
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    let mode = args
        .next()
        .ok_or("usage: video_stream listen|connect [endpoint-addr-b64]")?;
    let endpoint = Endpoint::builder(presets::N0)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await?;
    let address = URL_SAFE_NO_PAD.encode(postcard::to_stdvec(&endpoint.addr())?);
    println!("endpoint_addr_b64={address}");
    io::stdout().flush()?;

    match mode.as_str() {
        "listen" => receive_video(&endpoint).await?,
        "connect" => {
            let remote: EndpointAddr = postcard::from_bytes(
                &URL_SAFE_NO_PAD
                    .decode(args.next().ok_or("connect requires endpoint-addr-b64")?)?,
            )?;
            send_video(&endpoint, remote).await?;
        }
        _ => return Err("mode must be listen or connect".into()),
    }

    endpoint.close().await;
    Ok(())
}

async fn receive_video(endpoint: &Endpoint) -> Result<()> {
    let incoming = endpoint.accept().await.ok_or("endpoint closed")?;
    let connection = incoming.await?;
    let (mut send, mut receive) = connection.accept_bi().await?;
    let deadline = Instant::now() + RUN_FOR + Duration::from_secs(5);
    let mut frames = 0u64;
    let mut last_sequence = None;
    let mut sequence_tracker = media::SequenceTracker::default();

    while Instant::now() < deadline {
        let mut length = [0u8; media::MEDIA_FRAME_HEADER_BYTES];
        if tokio::time::timeout(Duration::from_secs(2), receive.read_exact(&mut length))
            .await
            .is_err()
        {
            break;
        }
        let length = u32::from_le_bytes(length) as usize;
        if length > media::MAX_MEDIA_PACKET_BYTES {
            return Err("media packet length exceeds limit".into());
        }
        let mut encoded = vec![0u8; length];
        receive.read_exact(&mut encoded).await?;
        let mut framed = (length as u32).to_le_bytes().to_vec();
        framed.append(&mut encoded);
        let packet =
            media::MediaPacket::decode_framed(&framed).map_err(|error| error.to_owned())?;
        sequence_tracker
            .accept(&packet)
            .map_err(|error| error.to_owned())?;
        if let Some(previous) = last_sequence
            && packet.sequence <= previous
        {
            return Err("video sequence moved backwards".into());
        }
        last_sequence = Some(packet.sequence);
        frames += 1;
        if frames == 1 {
            println!(
                "first frame: {}x{} ({} bytes)",
                packet.width,
                packet.height,
                packet.payload.len()
            );
        }
    }

    send.write_all(&frames.to_le_bytes()).await?;
    send.finish()?;
    connection.closed().await;
    println!("received {frames} video frames");
    Ok(())
}

async fn send_video(endpoint: &Endpoint, remote: EndpointAddr) -> Result<()> {
    let connection = endpoint.connect(remote, ALPN).await?;
    let (mut send, mut receive) = connection.open_bi().await?;
    let (sender, mut frames) = mpsc::channel(1);
    let stop = Arc::new(AtomicBool::new(false));
    let capture_stop = Arc::clone(&stop);
    let capture_thread = thread::spawn(move || -> Result<()> {
        let item = GraphicsCapturePicker::pick_item()?.ok_or("window selection canceled")?;
        let settings = Settings::new(
            item,
            CursorCaptureSettings::Default,
            DrawBorderSettings::Default,
            SecondaryWindowSettings::Default,
            MinimumUpdateIntervalSettings::Default,
            DirtyRegionSettings::Default,
            ColorFormat::Rgba8,
            (sender, capture_stop),
        );
        CaptureHandler::start(settings)?;
        Ok(())
    });
    let deadline = Instant::now() + RUN_FOR;
    let mut sequence = 0u64;

    while let Some(frame) = frames.recv().await {
        if Instant::now() >= deadline {
            break;
        }
        let packet = media::MediaPacket::video(
            1,
            sequence,
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_micros() as u64,
            frame.width,
            frame.height,
            frame.pixels,
        );
        let framed = match packet.encode_framed() {
            Ok(framed) if framed.len() <= media::MAX_MEDIA_FRAME_BYTES => framed,
            _ => continue,
        };
        send.write_all(&framed).await?;
        sequence += 1;
    }

    stop.store(true, Ordering::Relaxed);
    send.finish()?;
    let received = receive.read_to_end(8).await?;
    let count = received
        .get(..8)
        .and_then(|bytes| bytes.try_into().ok().map(u64::from_le_bytes))
        .unwrap_or(0);
    capture_thread
        .join()
        .map_err(|_| "capture thread panicked")??;
    println!("sent {sequence} video frames; receiver confirmed {count}");
    Ok(())
}
