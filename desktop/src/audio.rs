use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, SyncSender},
    },
    thread,
};
use wasapi::*;

pub const SAMPLE_RATE: usize = 48_000;
pub const CHANNELS: usize = 2;
const CHUNK_FRAMES: usize = 1024;

#[allow(dead_code)]
pub fn start_playback(
    receiver: Receiver<Vec<u8>>,
    stop: Arc<AtomicBool>,
) -> thread::JoinHandle<Result<(), String>> {
    thread::Builder::new()
        .name("seafriend-wasapi-playback".to_owned())
        .spawn(move || playback_loop(receiver, stop))
        .expect("failed to create audio playback thread")
}

pub fn start_process_capture(
    process_id: u32,
    sender: SyncSender<Vec<u8>>,
    stop: Arc<AtomicBool>,
) -> thread::JoinHandle<Result<(), String>> {
    thread::Builder::new()
        .name("seafriend-wasapi-process-loopback".to_owned())
        .spawn(move || capture_loop(process_id, sender, stop))
        .expect("failed to create audio capture thread")
}

fn capture_loop(
    process_id: u32,
    sender: SyncSender<Vec<u8>>,
    stop: Arc<AtomicBool>,
) -> Result<(), String> {
    initialize_mta()
        .ok()
        .map_err(|error| format!("COM initialization failed: {error:?}"))?;
    let format = WaveFormat::new(32, 32, SampleType::Float, SAMPLE_RATE, CHANNELS, None);
    let block_align = format.get_blockalign() as usize;
    let mut client = AudioClient::new_application_loopback_client(process_id, true)
        .map_err(|error| format!("audio loopback setup failed: {error:?}"))?;
    client
        .initialize_client(
            &format,
            Direction::Capture,
            StreamMode::EventsShared {
                autoconvert: true,
                buffer_duration_hns: 0,
            },
            None,
        )
        .map_err(|error| format!("audio client initialization failed: {error:?}"))?;
    let event = client
        .set_get_eventhandle()
        .map_err(|error| format!("audio event setup failed: {error:?}"))?;
    let capture = client
        .get_audiocaptureclient()
        .map_err(|error| format!("audio capture client failed: {error:?}"))?;
    let mut queued = VecDeque::new();
    client
        .start_stream()
        .map_err(|error| format!("audio stream start failed: {error:?}"))?;
    while !stop.load(Ordering::Relaxed) {
        while queued.len() >= block_align * CHUNK_FRAMES {
            let mut chunk = vec![0; block_align * CHUNK_FRAMES];
            for byte in &mut chunk {
                *byte = queued.pop_front().expect("audio chunk has enough bytes");
            }
            if sender.send(chunk).is_err() {
                client.stop_stream().ok();
                return Ok(());
            }
        }
        let frame_count = capture
            .get_next_packet_size()
            .map_err(|error| format!("audio packet query failed: {error:?}"))?
            .unwrap_or(0);
        if frame_count > 0 {
            let additional = (frame_count as usize * block_align)
                .saturating_sub(queued.capacity().saturating_sub(queued.len()));
            queued.reserve(additional);
            capture
                .read_from_device_to_deque(&mut queued)
                .map_err(|error| format!("audio packet read failed: {error:?}"))?;
        }
        event
            .wait_for_event(1000)
            .map_err(|error| format!("audio event wait failed: {error:?}"))?;
    }
    client.stop_stream().ok();
    Ok(())
}

fn playback_loop(receiver: Receiver<Vec<u8>>, stop: Arc<AtomicBool>) -> Result<(), String> {
    initialize_mta()
        .ok()
        .map_err(|error| format!("COM initialization failed: {error:?}"))?;
    let enumerator = DeviceEnumerator::new()
        .map_err(|error| format!("audio device enumeration failed: {error:?}"))?;
    let device = enumerator
        .get_default_device(Direction::Render)
        .map_err(|error| format!("default audio device unavailable: {error:?}"))?;
    let mut client = device
        .get_iaudioclient()
        .map_err(|error| format!("audio render client failed: {error:?}"))?;
    let format = WaveFormat::new(32, 32, SampleType::Float, SAMPLE_RATE, CHANNELS, None);
    let (_, minimum_period) = client
        .get_device_period()
        .map_err(|error| format!("audio device period failed: {error:?}"))?;
    client
        .initialize_client(
            &format,
            Direction::Render,
            StreamMode::EventsShared {
                autoconvert: true,
                buffer_duration_hns: minimum_period,
            },
            None,
        )
        .map_err(|error| format!("audio render initialization failed: {error:?}"))?;
    let event = client
        .set_get_eventhandle()
        .map_err(|error| format!("audio render event setup failed: {error:?}"))?;
    let render = client
        .get_audiorenderclient()
        .map_err(|error| format!("audio render client setup failed: {error:?}"))?;
    let block_align = format.get_blockalign() as usize;
    let mut queued = VecDeque::with_capacity(block_align * SAMPLE_RATE / 2);
    client
        .start_stream()
        .map_err(|error| format!("audio render stream start failed: {error:?}"))?;
    while !stop.load(Ordering::Relaxed) {
        let available = client
            .get_available_space_in_frames()
            .map_err(|error| format!("audio render buffer query failed: {error:?}"))?
            as usize;
        let required = available.saturating_mul(block_align);
        while queued.len() < required {
            match receiver.recv_timeout(std::time::Duration::from_millis(20)) {
                Ok(chunk) => queued.extend(chunk),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => queued.resize(required, 0),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    client.stop_stream().ok();
                    return Ok(());
                }
            }
        }
        render
            .write_to_device_from_deque(available, &mut queued, None)
            .map_err(|error| format!("audio render write failed: {error:?}"))?;
        event
            .wait_for_event(1000)
            .map_err(|error| format!("audio render event wait failed: {error:?}"))?;
    }
    client.stop_stream().ok();
    Ok(())
}
