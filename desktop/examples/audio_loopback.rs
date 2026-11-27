use std::{
    collections::VecDeque,
    env,
    error::Error,
    fs::File,
    io::Write,
    sync::mpsc::{self, SyncSender},
    thread,
    time::{Duration, Instant},
};
use wasapi::*;

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

const SAMPLE_RATE: usize = 48_000;
const CHANNELS: usize = 2;
const CHUNK_FRAMES: usize = 4096;

fn capture_loop(sender: SyncSender<Vec<u8>>, process_id: u32, duration: Duration) -> Result<()> {
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

    while started.elapsed() < duration {
        while queued.len() >= block_align * CHUNK_FRAMES {
            let mut chunk = vec![0; block_align * CHUNK_FRAMES];
            for byte in &mut chunk {
                *byte = queued.pop_front().expect("queued chunk has enough bytes");
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

fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    let process_id: u32 = args
        .next()
        .ok_or("usage: cargo run --example audio_loopback -- <pid> [seconds] [output.raw]")?
        .parse()?;
    let seconds: u64 = args
        .next()
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or(10);
    let output = args
        .next()
        .unwrap_or_else(|| "seafriend-audio.raw".to_owned());

    let (sender, receiver) = mpsc::sync_channel(2);
    let capture_thread = thread::Builder::new()
        .name("seafriend-wasapi-process-loopback".to_owned())
        .spawn(move || capture_loop(sender, process_id, Duration::from_secs(seconds)))?;

    let mut file = File::create(&output)?;
    let mut total_bytes = 0usize;
    while let Ok(chunk) = receiver.recv() {
        total_bytes += chunk.len();
        file.write_all(&chunk)?;
    }
    capture_thread
        .join()
        .map_err(|_| "capture thread panicked")??;
    file.flush()?;

    println!(
        "captured process {process_id}: {total_bytes} bytes at {SAMPLE_RATE} Hz/{CHANNELS} channels -> {output}"
    );
    Ok(())
}
