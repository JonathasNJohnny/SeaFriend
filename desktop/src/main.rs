mod api;
mod audio;
mod media;
mod signal;

use slint::{Image, Rgba8Pixel, SharedPixelBuffer, SharedString, Timer, TimerMode};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, SyncSender, TrySendError, channel, sync_channel},
    },
    time::{Duration, Instant},
};

#[allow(clippy::enum_variant_names)]
enum ApiCommand {
    CreateRoom { display_name: String },
    JoinRoom { code: String, display_name: String },
    LeaveRoom,
    CloseRoom,
}

enum ApiEvent {
    Ready(String),
    Status(String),
    Success {
        message: String,
        room_id: String,
        participant_id: String,
        participant_count: usize,
        participant_list: String,
    },
    Error(String),
}
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

slint::include_modules!();

#[derive(Clone)]
struct FrameFanout {
    ui: SyncSender<media::CapturedFrame>,
    network: MediaHub<media::CapturedFrame>,
}

#[derive(Clone)]
struct MediaHub<T> {
    destinations: Arc<Mutex<Vec<SyncSender<T>>>>,
}

impl<T> Default for MediaHub<T> {
    fn default() -> Self {
        Self {
            destinations: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl<T> MediaHub<T> {
    fn subscribe(&self) -> Receiver<T> {
        let (sender, receiver) = sync_channel(1);
        if let Ok(mut destinations) = self.destinations.lock() {
            destinations.push(sender);
        }
        receiver
    }
}

impl<T: Clone> MediaHub<T> {
    fn publish(&self, value: T) {
        if let Ok(mut destinations) = self.destinations.lock() {
            destinations.retain(|destination| match destination.try_send(value.clone()) {
                Ok(()) | Err(TrySendError::Full(_)) => true,
                Err(TrySendError::Disconnected(_)) => false,
            });
        }
    }
}

struct CaptureHandler {
    sender: FrameFanout,
    scratch: Vec<u8>,
    last_frame: Instant,
    frame_count: u64,
}

impl GraphicsCaptureApiHandler for CaptureHandler {
    type Flags = FrameFanout;
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self {
            sender: ctx.flags,
            scratch: Vec::new(),
            last_frame: Instant::now() - Duration::from_secs(1),
            frame_count: 0,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        _capture_control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        if self.last_frame.elapsed() >= Duration::from_millis(33) {
            let buffer = frame.buffer()?;
            let pixels = buffer.as_nopadding_buffer(&mut self.scratch).to_vec();
            let frame = media::CapturedFrame {
                width: buffer.width(),
                height: buffer.height(),
                pixels,
            };
            let _ = self.sender.ui.try_send(frame.clone());
            self.sender.network.publish(frame);
            self.last_frame = Instant::now();
        }
        self.frame_count += 1;
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        eprintln!("Capture session ended after {} frames", self.frame_count);
        Ok(())
    }
}

fn start_capture(status: Arc<Mutex<String>>, sender: FrameFanout) {
    std::thread::spawn(move || {
        let result = (|| -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            set_status(&status, "selecionando janela".to_owned());
            let item = GraphicsCapturePicker::pick_item()?;
            let Some(item) = item else {
                set_status(&status, "seleção cancelada".to_owned());
                return Ok(());
            };
            let settings = Settings::new(
                item,
                CursorCaptureSettings::Default,
                DrawBorderSettings::Default,
                SecondaryWindowSettings::Default,
                MinimumUpdateIntervalSettings::Default,
                DirtyRegionSettings::Default,
                ColorFormat::Rgba8,
                sender,
            );
            set_status(&status, "capturando".to_owned());
            CaptureHandler::start(settings)?;
            Ok(())
        })();
        if let Err(error) = result {
            set_status(&status, format!("erro: {error}"));
        }
    });
}

fn set_status(status: &Arc<Mutex<String>>, value: String) {
    if let Ok(mut current) = status.lock() {
        *current = value;
    }
}

#[allow(dead_code, clippy::too_many_arguments)]
fn activate_room(
    response: api::RoomResponse,
    base_url: &str,
    events: &std::sync::mpsc::Sender<ApiEvent>,
    signal_events: &std::sync::mpsc::Sender<String>,
    media_hub: &MediaHub<media::CapturedFrame>,
    display_frames: &SyncSender<media::CapturedFrame>,
    audio_hub: &MediaHub<Vec<u8>>,
    audio_playback: &SyncSender<Vec<u8>>,
) -> (String, String) {
    let code = response
        .access_code
        .unwrap_or_else(|| "código existente".to_owned());
    let invite = response.invite_link.unwrap_or_default();
    let connected = response
        .room
        .participants
        .len()
        .max(response.room.participant_count);
    let room_id = response.room.id.clone();
    let participant_id = response.participant.id.clone();
    let participant_list = response
        .room
        .participants
        .iter()
        .map(|participant| participant.display_name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let _ = events.send(ApiEvent::Success {
        message: format!(
            "Sala {} — {}/{} participante(s) — você é {} — código: {} {}",
            response.room.id,
            connected,
            response.room.max_participants,
            if response.participant.is_host {
                "host"
            } else {
                "participante"
            },
            code,
            if invite.is_empty() {
                format!("({})", response.participant.display_name)
            } else {
                format!("— {invite} ({})", response.participant.display_name)
            }
        ),
        room_id: room_id.clone(),
        participant_id: participant_id.clone(),
        participant_count: connected,
        participant_list,
    });
    signal::start(
        base_url.to_owned(),
        room_id.clone(),
        participant_id.clone(),
        signal_events.clone(),
        media_hub.subscribe(),
        display_frames.clone(),
        audio_hub.subscribe(),
        audio_playback.clone(),
    );
    (room_id, participant_id)
}

#[allow(clippy::too_many_arguments)]
fn start_api_worker(
    base_url: String,
    commands: Receiver<ApiCommand>,
    events: std::sync::mpsc::Sender<ApiEvent>,
    signal_events: std::sync::mpsc::Sender<String>,
    media_hub: MediaHub<media::CapturedFrame>,
    display_frames: SyncSender<media::CapturedFrame>,
    audio_hub: MediaHub<Vec<u8>>,
    audio_playback: SyncSender<Vec<u8>>,
) {
    std::thread::spawn(move || {
        let runtime = match tokio::runtime::Runtime::new() {
            Ok(runtime) => runtime,
            Err(error) => {
                let _ = events.send(ApiEvent::Error(format!("runtime error: {error}")));
                return;
            }
        };
        let client = api::ApiClient::new(base_url.clone());
        let _ = events.send(ApiEvent::Ready("API pronta".to_owned()));
        let mut session: Option<(String, String)> = None;
        let mut signal_stop: Option<Arc<AtomicBool>> = None;
        while let Ok(command) = commands.recv() {
            match command {
                command @ (ApiCommand::CreateRoom { .. } | ApiCommand::JoinRoom { .. }) => {
                    if session.is_some() {
                        let _ = events.send(ApiEvent::Error(
                            "já existe uma sala ativa; saia antes de entrar em outra".to_owned(),
                        ));
                        continue;
                    }
                    let result = runtime.block_on(async {
                        match &command {
                            ApiCommand::CreateRoom { display_name } => {
                                client.create_room(display_name).await
                            }
                            ApiCommand::JoinRoom { code, display_name } => {
                                client.join_room(code, display_name).await
                            }
                            _ => unreachable!(),
                        }
                    });
                    match result {
                        Ok(response) => {
                            let code = response
                                .access_code
                                .unwrap_or_else(|| "código existente".to_owned());
                            let invite = response.invite_link.unwrap_or_default();
                            let connected = response
                                .room
                                .participants
                                .len()
                                .max(response.room.participant_count);
                            let room_id = response.room.id.clone();
                            let participant_id = response.participant.id.clone();
                            let participant_list = response
                                .room
                                .participants
                                .iter()
                                .map(|participant| participant.display_name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ");
                            let _ = events.send(ApiEvent::Success {
                                message: format!(
                                    "Sala {} — {}/{} participante(s) — você é {} — código: {} {}",
                                    response.room.id,
                                    connected,
                                    response.room.max_participants,
                                    if response.participant.is_host {
                                        "host"
                                    } else {
                                        "participante"
                                    },
                                    code,
                                    if invite.is_empty() {
                                        format!("({})", response.participant.display_name)
                                    } else {
                                        format!(
                                            "— {invite} ({})",
                                            response.participant.display_name
                                        )
                                    }
                                ),
                                room_id: room_id.clone(),
                                participant_id: participant_id.clone(),
                                participant_count: connected,
                                participant_list,
                            });
                            session = Some((room_id.clone(), participant_id.clone()));
                            signal_stop = Some(signal::start(
                                base_url.clone(),
                                room_id,
                                participant_id,
                                signal_events.clone(),
                                media_hub.subscribe(),
                                display_frames.clone(),
                                audio_hub.subscribe(),
                                audio_playback.clone(),
                            ));
                        }
                        Err(error) => {
                            let _ = events.send(ApiEvent::Error(error));
                        }
                    }
                }
                ApiCommand::LeaveRoom | ApiCommand::CloseRoom => {
                    let Some((room_id, participant_id)) = session.clone() else {
                        let _ = events.send(ApiEvent::Error("nenhuma sala ativa".to_owned()));
                        continue;
                    };
                    let is_close = matches!(command, ApiCommand::CloseRoom);
                    let result = runtime.block_on(async {
                        if is_close {
                            client.close_room(&room_id, &participant_id).await
                        } else {
                            client.leave_room(&room_id, &participant_id).await
                        }
                    });
                    match result {
                        Ok(()) => {
                            session = None;
                            if let Some(stop) = signal_stop.take() {
                                stop.store(true, Ordering::Relaxed);
                            }
                            let _ = events.send(ApiEvent::Status(if is_close {
                                "sala encerrada".to_owned()
                            } else {
                                "você saiu da sala".to_owned()
                            }));
                        }
                        Err(error) => {
                            let _ = events.send(ApiEvent::Error(error));
                        }
                    }
                }
            }
        }
    });
}

fn main() -> Result<(), slint::PlatformError> {
    let window = MainWindow::new()?;
    let status = Arc::new(Mutex::new(String::from("aguardando seleção")));
    let (display_sender, display_receiver) = sync_channel(1);
    let media_hub = MediaHub::default();
    let (audio_sender, audio_capture_receiver) = sync_channel(4);
    let audio_hub = MediaHub::default();
    let audio_hub_dispatch = audio_hub.clone();
    std::thread::spawn(move || {
        while let Ok(chunk) = audio_capture_receiver.recv() {
            audio_hub_dispatch.publish(chunk);
        }
    });
    let audio_stop = Arc::new(AtomicBool::new(false));
    let audio_capture_started = Arc::new(AtomicBool::new(false));
    let (audio_playback_sender, audio_playback_receiver) = sync_channel(4);
    let audio_playback_stop = Arc::new(AtomicBool::new(false));
    let _audio_playback =
        audio::start_playback(audio_playback_receiver, audio_playback_stop.clone());
    let audio_sender_for_env = audio_sender.clone();
    let audio_capture_started_for_env = audio_capture_started.clone();
    let _audio_capture = std::env::var("SEAFRIEND_AUDIO_PROCESS_ID")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .map(|process_id| {
            audio_capture_started_for_env.store(true, Ordering::Relaxed);
            audio::start_process_capture(process_id, audio_sender_for_env, audio_stop.clone())
        });
    let (api_commands, api_command_receiver) = channel();
    let (api_event_sender, api_event_receiver) = channel();
    let (signal_event_sender, signal_event_receiver) = channel();
    let api_base_url =
        std::env::var("SEAFRIEND_API_URL").unwrap_or_else(|_| "http://127.0.0.1:3100".to_owned());
    start_api_worker(
        api_base_url,
        api_command_receiver,
        api_event_sender,
        signal_event_sender,
        media_hub.clone(),
        display_sender.clone(),
        audio_hub.clone(),
        audio_playback_sender,
    );

    let create_commands = api_commands.clone();
    let create_window = window.as_weak();
    window.on_create_room(move || {
        if let Some(window) = create_window.upgrade() {
            let _ = create_commands.send(ApiCommand::CreateRoom {
                display_name: window.get_display_name().to_string(),
            });
        }
    });
    let join_commands = api_commands.clone();
    let join_window = window.as_weak();
    window.on_join_room(move || {
        if let Some(window) = join_window.upgrade() {
            let _ = join_commands.send(ApiCommand::JoinRoom {
                code: window.get_join_code().to_string(),
                display_name: window.get_display_name().to_string(),
            });
        }
    });
    let leave_commands = api_commands.clone();
    window.on_leave_room(move || {
        let _ = leave_commands.send(ApiCommand::LeaveRoom);
    });
    let close_commands = api_commands.clone();
    window.on_close_room(move || {
        let _ = close_commands.send(ApiCommand::CloseRoom);
    });
    let capture_status = Arc::clone(&status);
    let capture_fanout = FrameFanout {
        ui: display_sender.clone(),
        network: media_hub,
    };
    window.on_select_window(move || {
        start_capture(Arc::clone(&capture_status), capture_fanout.clone());
    });
    let audio_status = Arc::clone(&status);
    let audio_sender_for_ui = audio_sender.clone();
    let audio_stop_for_ui = audio_stop.clone();
    let audio_window = window.as_weak();
    window.on_start_audio(move || {
        let Some(window) = audio_window.upgrade() else {
            return;
        };
        let process_id = window.get_audio_process_id().to_string().parse::<u32>();
        let Ok(process_id) = process_id else {
            set_status(&audio_status, "PID de áudio inválido".to_owned());
            return;
        };
        if audio_capture_started.swap(true, Ordering::Relaxed) {
            set_status(&audio_status, "captura de áudio já iniciada".to_owned());
            return;
        }
        audio_stop_for_ui.store(false, Ordering::Relaxed);
        let _ = audio::start_process_capture(
            process_id,
            audio_sender_for_ui.clone(),
            audio_stop_for_ui.clone(),
        );
        set_status(
            &audio_status,
            format!("capturando áudio do processo {process_id}"),
        );
    });
    let process_window = window.as_weak();
    window.on_list_audio_processes(move || {
        let process_window = process_window.clone();
        std::thread::spawn(move || {
            let output = std::process::Command::new("tasklist")
                .args(["/FO", "CSV", "/NH"])
                .output();
            let text = match output {
                Ok(output) if output.status.success() => {
                    let mut lines = String::from_utf8_lossy(&output.stdout)
                        .lines()
                        .take(20)
                        .map(|line| line.replace('"', ""))
                        .collect::<Vec<_>>();
                    if lines.is_empty() {
                        "nenhum processo encontrado".to_owned()
                    } else {
                        lines.insert(0, "Processos (nome, PID):".to_owned());
                        lines.join("\n")
                    }
                }
                Ok(output) => format!("falha ao listar processos ({})", output.status),
                Err(error) => format!("não foi possível listar processos: {error}"),
            };
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(window) = process_window.upgrade() {
                    window.set_audio_processes(SharedString::from(text));
                }
            });
        });
    });

    let timer_window = window.as_weak();
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(33), move || {
        let Some(window) = timer_window.upgrade() else {
            return;
        };
        if let Ok(current_status) = status.lock() {
            window.set_capture_status(SharedString::from(current_status.as_str()));
        }

        while let Ok(event) = api_event_receiver.try_recv() {
            match event {
                ApiEvent::Ready(message) | ApiEvent::Error(message) => {
                    window.set_api_status(SharedString::from(message));
                }
                ApiEvent::Status(message) => {
                    window.set_api_status(SharedString::from(message));
                    window.set_room_id(SharedString::new());
                    window.set_participant_id(SharedString::new());
                    window.set_participant_count(0);
                    window.set_participant_list(SharedString::new());
                }
                ApiEvent::Success {
                    message,
                    room_id,
                    participant_id,
                    participant_count,
                    participant_list,
                } => {
                    window.set_api_status(SharedString::from(message));
                    window.set_room_id(SharedString::from(room_id));
                    window.set_participant_id(SharedString::from(participant_id));
                    window.set_participant_count(participant_count as i32);
                    window.set_participant_list(SharedString::from(participant_list));
                }
            }
        }
        while let Ok(message) = signal_event_receiver.try_recv() {
            if message.contains("participant_connected") {
                window.set_participant_count(window.get_participant_count() + 1);
            } else if message.contains("participant_disconnected") {
                window.set_participant_count((window.get_participant_count() - 1).max(0));
            }
            window.set_api_status(SharedString::from(message));
        }

        let mut latest = None;
        while let Ok(frame) = display_receiver.try_recv() {
            latest = Some(frame);
        }
        if let Some(frame) = latest {
            let expected_len = (frame.width as usize)
                .saturating_mul(frame.height as usize)
                .saturating_mul(4);
            if frame.pixels.len() == expected_len {
                let pixels = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                    &frame.pixels,
                    frame.width,
                    frame.height,
                );
                window.set_capture_image(Image::from_rgba8(pixels));
            }
        }
    });

    let result = window.run();
    audio_stop.store(true, Ordering::Relaxed);
    audio_playback_stop.store(true, Ordering::Relaxed);
    result
}
