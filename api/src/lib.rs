mod rooms;

use axum::{
    Json, Router,
    extract::{
        Path, Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::{SinkExt, StreamExt};
use rooms::{RoomError, RoomManager, RoomSnapshot};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::broadcast;
use tower_http::{cors::CorsLayer, trace::TraceLayer};

#[derive(Clone)]
pub struct AppState {
    rooms: Arc<RoomManager>,
    signaling: Arc<SignalHub>,
}

#[derive(Default)]
struct SignalHub {
    channels: Mutex<HashMap<uuid::Uuid, broadcast::Sender<SignalEvent>>>,
}

#[derive(Debug, Clone, Serialize)]
struct SignalEvent {
    kind: String,
    participant_id: Option<uuid::Uuid>,
    from_participant_id: Option<uuid::Uuid>,
    data: Option<serde_json::Value>,
}

impl AppState {
    pub fn from_env() -> Self {
        let expiration = std::env::var("SEAFRIEND_ROOM_TTL_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .map(Duration::from_secs)
            .unwrap_or_else(|| Duration::from_secs(60 * 60));

        let cleanup_interval = std::env::var("SEAFRIEND_CLEANUP_INTERVAL_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .map(Duration::from_secs)
            .unwrap_or_else(|| Duration::from_secs(30));

        let presence_timeout = std::env::var("SEAFRIEND_PRESENCE_TIMEOUT_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .map(Duration::from_secs)
            .unwrap_or_else(|| Duration::from_secs(60));

        let invite_base_url = std::env::var("SEAFRIEND_INVITE_BASE_URL")
            .unwrap_or_else(|_| "http://localhost:3100".to_owned());

        Self {
            rooms: Arc::new(RoomManager::with_presence_timeout(
                expiration,
                cleanup_interval,
                presence_timeout,
                invite_base_url,
            )),
            signaling: Arc::new(SignalHub::default()),
        }
    }

    pub fn for_tests(expiration: Duration) -> Self {
        Self {
            rooms: Arc::new(RoomManager::new(
                expiration,
                Duration::from_secs(1),
                "http://localhost:3100".to_owned(),
            )),
            signaling: Arc::new(SignalHub::default()),
        }
    }

    pub fn cleanup_interval(&self) -> Duration {
        self.rooms.cleanup_interval()
    }

    pub async fn cleanup_expired(&self) {
        self.rooms.cleanup_expired().await;
    }
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/rooms", post(create_room))
        .route("/rooms/join", post(join_by_code))
        .route("/rooms/invite/{invite_token}/join", post(join_by_invite))
        .route("/rooms/{room_id}", get(get_room))
        .route("/rooms/{room_id}/participants", get(get_participants))
        .route("/rooms/{room_id}/leave", post(leave_room))
        .route("/rooms/{room_id}/disconnect", post(disconnect_room))
        .route("/rooms/{room_id}/close", post(close_room))
        .route("/rooms/{room_id}/signal", get(signal_socket))
        .with_state(state)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({
        "status": "online",
        "service": "SeaFriend API",
        "version": "0.1.0"
    }))
}

#[derive(Debug, Deserialize)]
struct CreateRoomRequest {
    display_name: String,
}

#[derive(Debug, Deserialize)]
struct JoinByCodeRequest {
    code: String,
    display_name: String,
}

#[derive(Debug, Deserialize)]
struct JoinByInviteRequest {
    display_name: String,
}

#[derive(Debug, Deserialize)]
struct ParticipantQuery {
    participant_id: uuid::Uuid,
}

#[derive(Debug, Deserialize)]
struct ParticipantBody {
    participant_id: uuid::Uuid,
}

#[derive(Debug, Serialize)]
struct CreateRoomResponse {
    room: RoomSnapshot,
    participant: rooms::ParticipantSnapshot,
    access_code: String,
    invite_link: String,
}

#[derive(Debug, Serialize)]
struct JoinRoomResponse {
    room: RoomSnapshot,
    participant: rooms::ParticipantSnapshot,
}

async fn create_room(
    State(state): State<AppState>,
    Json(payload): Json<CreateRoomRequest>,
) -> Result<(StatusCode, Json<CreateRoomResponse>), ApiError> {
    let created = state.rooms.create_room(&payload.display_name).await?;
    Ok((
        StatusCode::CREATED,
        Json(CreateRoomResponse {
            room: created.room,
            participant: created.participant,
            access_code: created.access_code,
            invite_link: created.invite_link,
        }),
    ))
}

async fn join_by_code(
    State(state): State<AppState>,
    Json(payload): Json<JoinByCodeRequest>,
) -> Result<Json<JoinRoomResponse>, ApiError> {
    let joined = state
        .rooms
        .join_by_code(&payload.code, &payload.display_name)
        .await?;
    Ok(Json(JoinRoomResponse {
        room: joined.room,
        participant: joined.participant,
    }))
}

async fn join_by_invite(
    State(state): State<AppState>,
    Path(invite_token): Path<String>,
    Json(payload): Json<JoinByInviteRequest>,
) -> Result<Json<JoinRoomResponse>, ApiError> {
    let joined = state
        .rooms
        .join_by_invite(&invite_token, &payload.display_name)
        .await?;
    Ok(Json(JoinRoomResponse {
        room: joined.room,
        participant: joined.participant,
    }))
}

async fn get_room(
    State(state): State<AppState>,
    Path(room_id): Path<uuid::Uuid>,
    Query(query): Query<ParticipantQuery>,
) -> Result<Json<RoomSnapshot>, ApiError> {
    Ok(Json(
        state.rooms.get_room(room_id, query.participant_id).await?,
    ))
}

async fn get_participants(
    State(state): State<AppState>,
    Path(room_id): Path<uuid::Uuid>,
    Query(query): Query<ParticipantQuery>,
) -> Result<Json<Vec<rooms::ParticipantSnapshot>>, ApiError> {
    Ok(Json(
        state
            .rooms
            .get_participants(room_id, query.participant_id)
            .await?,
    ))
}

async fn leave_room(
    State(state): State<AppState>,
    Path(room_id): Path<uuid::Uuid>,
    Json(payload): Json<ParticipantBody>,
) -> Result<StatusCode, ApiError> {
    state.rooms.leave(room_id, payload.participant_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn disconnect_room(
    State(state): State<AppState>,
    Path(room_id): Path<uuid::Uuid>,
    Json(payload): Json<ParticipantBody>,
) -> Result<StatusCode, ApiError> {
    state
        .rooms
        .disconnect(room_id, payload.participant_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn close_room(
    State(state): State<AppState>,
    Path(room_id): Path<uuid::Uuid>,
    Json(payload): Json<ParticipantBody>,
) -> Result<StatusCode, ApiError> {
    state.rooms.close(room_id, payload.participant_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
struct ClientSignal {
    kind: String,
    data: Option<serde_json::Value>,
}

impl ClientSignal {
    fn validate(&self) -> Result<(), &'static str> {
        match self.kind.as_str() {
            "heartbeat" if self.data.is_none() => Ok(()),
            "signal" if self.data.is_some() => Ok(()),
            _ => Err("unsupported signal message"),
        }
    }
}

impl SignalHub {
    fn subscribe(&self, room_id: uuid::Uuid) -> broadcast::Receiver<SignalEvent> {
        let mut channels = self.channels.lock().expect("signal hub lock poisoned");
        channels
            .entry(room_id)
            .or_insert_with(|| broadcast::channel(64).0)
            .subscribe()
    }

    fn send(&self, room_id: uuid::Uuid, event: SignalEvent) {
        let channels = self.channels.lock().expect("signal hub lock poisoned");
        if let Some(sender) = channels.get(&room_id) {
            let _ = sender.send(event);
        }
    }
}

async fn signal_socket(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(room_id): Path<uuid::Uuid>,
    Query(query): Query<ParticipantQuery>,
) -> Result<Response, ApiError> {
    let room = state.rooms.get_room(room_id, query.participant_id).await?;
    let receiver = state.signaling.subscribe(room_id);
    let participant_id = query.participant_id;
    let initial = SignalEvent {
        kind: "presence_snapshot".to_owned(),
        participant_id: Some(participant_id),
        from_participant_id: None,
        data: Some(serde_json::to_value(room.participants).expect("snapshot serializes")),
    };
    let hub = Arc::clone(&state.signaling);
    let rooms = Arc::clone(&state.rooms);

    Ok(ws.on_upgrade(move |socket| async move {
        handle_signal_socket(
            socket,
            room_id,
            participant_id,
            initial,
            receiver,
            hub,
            rooms,
        )
        .await;
    }))
}

async fn handle_signal_socket(
    socket: WebSocket,
    room_id: uuid::Uuid,
    participant_id: uuid::Uuid,
    initial: SignalEvent,
    mut receiver: broadcast::Receiver<SignalEvent>,
    hub: Arc<SignalHub>,
    rooms: Arc<RoomManager>,
) {
    let (mut outgoing, mut incoming) = socket.split();
    if send_event(&mut outgoing, &initial).await.is_err() {
        return;
    }
    hub.send(
        room_id,
        SignalEvent {
            kind: "participant_connected".to_owned(),
            participant_id: Some(participant_id),
            from_participant_id: None,
            data: None,
        },
    );

    loop {
        tokio::select! {
            message = incoming.next() => {
                match message {
                    Some(Ok(Message::Text(text))) => {
                        let Ok(client_message) = serde_json::from_str::<ClientSignal>(&text) else {
                            continue;
                        };
                        if client_message.validate().is_err() {
                            continue;
                        }
                        if client_message.kind == "heartbeat" {
                            if rooms.heartbeat(room_id, participant_id).await.is_err() {
                                break;
                            }
                            let event = SignalEvent {
                                kind: "heartbeat_ack".to_owned(),
                                participant_id: Some(participant_id),
                                from_participant_id: None,
                                data: None,
                            };
                            if send_event(&mut outgoing, &event).await.is_err() {
                                break;
                            }
                        } else if client_message.kind == "signal" {
                            hub.send(room_id, SignalEvent {
                                kind: "signal".to_owned(),
                                participant_id: None,
                                from_participant_id: Some(participant_id),
                                data: client_message.data,
                            });
                        }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => {}
                }
            }
            event = receiver.recv() => {
                match event {
                    Ok(event)
                        if (event.participant_id != Some(participant_id)
                            && event.from_participant_id != Some(participant_id))
                            || event.kind == "heartbeat_ack" => {
                        if send_event(&mut outgoing, &event).await.is_err() {
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }

    let _ = rooms.signal_disconnected(room_id, participant_id).await;
    hub.send(
        room_id,
        SignalEvent {
            kind: "participant_disconnected".to_owned(),
            participant_id: Some(participant_id),
            from_participant_id: None,
            data: None,
        },
    );
}

async fn send_event(
    outgoing: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    event: &SignalEvent,
) -> Result<(), axum::Error> {
    let payload = serde_json::to_string(event).expect("signal event serializes");
    outgoing.send(Message::Text(payload.into())).await
}

#[derive(Debug)]
struct ApiError(RoomError);

impl From<RoomError> for ApiError {
    fn from(error: RoomError) -> Self {
        Self(error)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self.0 {
            RoomError::InvalidDisplayName => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_display_name",
                "display_name must contain between 1 and 64 characters",
            ),
            RoomError::NotFound | RoomError::InvalidCodeOrInvite => {
                (StatusCode::NOT_FOUND, "room_not_found", "room not found")
            }
            RoomError::ClosedOrExpired => {
                (StatusCode::GONE, "room_closed", "room is closed or expired")
            }
            RoomError::Full => (StatusCode::CONFLICT, "room_full", "room is full"),
            RoomError::ParticipantNotFound => (
                StatusCode::NOT_FOUND,
                "participant_not_found",
                "participant not found",
            ),
            RoomError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "participant is not authorized for this room",
            ),
            RoomError::Forbidden => (
                StatusCode::FORBIDDEN,
                "forbidden",
                "operation is not allowed",
            ),
        };

        (status, Json(json!({ "error": code, "message": message }))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use futures_util::{SinkExt, StreamExt};
    use serde_json::Value;
    use tokio_tungstenite::{connect_async, tungstenite::Message};
    use tower::ServiceExt;

    fn json_request(method: &str, uri: &str, body: Value) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    async fn json_body(response: axum::response::Response) -> Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn health_remains_available() {
        let response = build_router(AppState::for_tests(Duration::from_secs(60)))
            .oneshot(Request::get("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn creates_and_joins_room_by_code() {
        let app = build_router(AppState::for_tests(Duration::from_secs(60)));
        let response = app
            .clone()
            .oneshot(json_request(
                "POST",
                "/rooms",
                json!({ "display_name": "Host" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let created = json_body(response).await;
        let code = created["access_code"].as_str().unwrap();
        let room_id = created["room"]["id"].as_str().unwrap();
        let host_id = created["participant"]["id"].as_str().unwrap();
        assert_eq!(created["room"]["participant_count"], 1);
        assert!(created["invite_link"].as_str().unwrap().contains("/join/"));

        let response = app
            .clone()
            .oneshot(json_request(
                "POST",
                "/rooms/join",
                json!({ "code": code, "display_name": "Guest" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let joined = json_body(response).await;
        assert_eq!(joined["room"]["id"], room_id);
        assert_ne!(joined["participant"]["id"], host_id);
        assert_eq!(joined["room"]["participant_count"], 2);

        let invite_link = created["invite_link"].as_str().unwrap();
        let invite_token = invite_link.rsplit('/').next().unwrap();
        let response = app
            .oneshot(json_request(
                "POST",
                &format!("/rooms/invite/{invite_token}/join"),
                json!({ "display_name": "Invite guest" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let invited = json_body(response).await;
        assert_eq!(invited["room"]["participant_count"], 3);
    }

    #[tokio::test]
    async fn rejects_invalid_code_and_unauthorized_query() {
        let app = build_router(AppState::for_tests(Duration::from_secs(60)));
        let response = app
            .clone()
            .oneshot(json_request(
                "POST",
                "/rooms/join",
                json!({ "code": "invalid", "display_name": "Guest" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let response = app
            .oneshot(
                Request::get(format!(
                    "/rooms/{}/participants?participant_id={}",
                    uuid::Uuid::new_v4(),
                    uuid::Uuid::new_v4()
                ))
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn leaves_and_closes_room() {
        let app = build_router(AppState::for_tests(Duration::from_secs(60)));
        let response = app
            .clone()
            .oneshot(json_request(
                "POST",
                "/rooms",
                json!({ "display_name": "Host" }),
            ))
            .await
            .unwrap();
        let created = json_body(response).await;
        let room_id = created["room"]["id"].as_str().unwrap();
        let host_id = created["participant"]["id"].as_str().unwrap();

        let response = app
            .clone()
            .oneshot(json_request(
                "POST",
                "/rooms/join",
                json!({ "code": created["access_code"], "display_name": "Guest" }),
            ))
            .await
            .unwrap();
        let guest = json_body(response).await;
        let guest_id = guest["participant"]["id"].as_str().unwrap();

        let response = app
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/rooms/{room_id}/leave"),
                json!({ "participant_id": guest_id }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let response = app
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/rooms/{room_id}/close"),
                json!({ "participant_id": host_id }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let response = app
            .oneshot(json_request(
                "POST",
                "/rooms/join",
                json!({ "code": created["access_code"], "display_name": "Guest" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::GONE);
    }

    #[tokio::test]
    async fn expires_room_automatically_on_access() {
        let manager = RoomManager::new(
            Duration::from_millis(1),
            Duration::from_secs(1),
            "http://localhost:3100".to_owned(),
        );
        let created = manager.create_room("Host").await.unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(matches!(
            manager.join_by_code(&created.access_code, "Guest").await,
            Err(RoomError::ClosedOrExpired)
        ));
    }

    #[tokio::test]
    async fn concurrent_entries_cannot_exceed_capacity() {
        let manager = Arc::new(RoomManager::new(
            Duration::from_secs(60),
            Duration::from_secs(1),
            "http://localhost:3100".to_owned(),
        ));
        let created = manager.create_room("Host").await.unwrap();
        let mut tasks = Vec::new();
        for index in 0..12 {
            let manager = Arc::clone(&manager);
            let code = created.access_code.clone();
            tasks.push(tokio::spawn(async move {
                manager.join_by_code(&code, &format!("Guest-{index}")).await
            }));
        }

        let mut successful = 0;
        for task in tasks {
            if task.await.unwrap().is_ok() {
                successful += 1;
            }
        }
        assert_eq!(successful, 9);
        let room = manager
            .get_room(created.room.id, created.participant.id)
            .await
            .unwrap();
        assert_eq!(room.participant_count, 10);
    }

    #[test]
    fn signal_messages_require_the_expected_payload() {
        assert!(
            ClientSignal {
                kind: "heartbeat".to_owned(),
                data: None,
            }
            .validate()
            .is_ok()
        );
        assert!(
            ClientSignal {
                kind: "signal".to_owned(),
                data: Some(json!({ "type": "offer" })),
            }
            .validate()
            .is_ok()
        );
        assert!(
            ClientSignal {
                kind: "heartbeat".to_owned(),
                data: Some(json!({ "unexpected": true })),
            }
            .validate()
            .is_err()
        );
        assert!(
            ClientSignal {
                kind: "unknown".to_owned(),
                data: None,
            }
            .validate()
            .is_err()
        );
    }

    #[tokio::test]
    async fn two_websocket_clients_receive_presence_and_heartbeat() {
        let state = AppState::for_tests(Duration::from_secs(60));
        let created = state.rooms.create_room("Host").await.unwrap();
        let guest = state
            .rooms
            .join_by_code(&created.access_code, "Guest")
            .await
            .unwrap();
        let app = build_router(state);
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let room_id = created.room.id;
        let host_id = created.participant.id;
        let guest_id = guest.participant.id;
        let host_url = format!("ws://{address}/rooms/{room_id}/signal?participant_id={host_id}");
        let guest_url = format!("ws://{address}/rooms/{room_id}/signal?participant_id={guest_id}");
        let (mut host, _) = connect_async(host_url.clone()).await.unwrap();
        let host_snapshot = host.next().await.unwrap().unwrap();
        let Message::Text(host_snapshot) = host_snapshot else {
            panic!("host did not receive a text snapshot");
        };
        assert_eq!(
            serde_json::from_str::<Value>(&host_snapshot).unwrap()["kind"],
            "presence_snapshot"
        );

        host.close(None).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        let (mut reconnected_host, _) = connect_async(host_url.clone()).await.unwrap();
        let reconnected_snapshot = reconnected_host.next().await.unwrap().unwrap();
        let Message::Text(reconnected_snapshot) = reconnected_snapshot else {
            panic!("reconnected host did not receive a text snapshot");
        };
        assert_eq!(
            serde_json::from_str::<Value>(&reconnected_snapshot).unwrap()["kind"],
            "presence_snapshot"
        );

        let (mut guest, _) = connect_async(guest_url).await.unwrap();
        let guest_snapshot = guest.next().await.unwrap().unwrap();
        let Message::Text(guest_snapshot) = guest_snapshot else {
            panic!("guest did not receive a text snapshot");
        };
        assert_eq!(
            serde_json::from_str::<Value>(&guest_snapshot).unwrap()["kind"],
            "presence_snapshot"
        );

        let connected = reconnected_host.next().await.unwrap().unwrap();
        let Message::Text(connected) = connected else {
            panic!("host did not receive participant_connected");
        };
        let connected: Value = serde_json::from_str(&connected).unwrap();
        assert_eq!(connected["kind"], "participant_connected");
        assert_eq!(connected["participant_id"], guest_id.to_string());

        guest
            .send(Message::Text(
                json!({ "kind": "heartbeat" }).to_string().into(),
            ))
            .await
            .unwrap();
        let heartbeat = guest.next().await.unwrap().unwrap();
        let Message::Text(heartbeat) = heartbeat else {
            panic!("guest did not receive heartbeat_ack");
        };
        assert_eq!(
            serde_json::from_str::<Value>(&heartbeat).unwrap()["kind"],
            "heartbeat_ack"
        );

        guest
            .send(Message::Text(
                json!({
                    "kind": "signal",
                    "data": { "type": "endpoint", "address": "test" }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        let relayed = reconnected_host.next().await.unwrap().unwrap();
        let Message::Text(relayed) = relayed else {
            panic!("host did not receive relayed signal");
        };
        let relayed: Value = serde_json::from_str(&relayed).unwrap();
        assert_eq!(relayed["kind"], "signal");
        assert_eq!(relayed["from_participant_id"], guest_id.to_string());
        assert!(
            tokio::time::timeout(Duration::from_millis(100), guest.next())
                .await
                .is_err()
        );

        reconnected_host.close(None).await.unwrap();
        guest.close(None).await.unwrap();
        server.abort();
    }

    #[tokio::test]
    async fn cleanup_removes_stale_guests_but_closes_stale_hosts() {
        let manager = RoomManager::with_presence_timeout(
            Duration::from_secs(60),
            Duration::from_secs(1),
            Duration::from_secs(1),
            "http://localhost:3100".to_owned(),
        );
        let created = manager.create_room("Host").await.unwrap();
        let guest = manager
            .join_by_code(&created.access_code, "Guest")
            .await
            .unwrap();

        manager
            .disconnect(created.room.id, guest.participant.id)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(1_100)).await;
        manager
            .heartbeat(created.room.id, created.participant.id)
            .await
            .unwrap();
        manager.cleanup_expired().await;

        let room = manager
            .get_room(created.room.id, created.participant.id)
            .await
            .unwrap();
        assert_eq!(room.participant_count, 1);

        manager
            .disconnect(created.room.id, created.participant.id)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(1_100)).await;
        manager.cleanup_expired().await;
        assert!(matches!(
            manager
                .get_room(created.room.id, created.participant.id)
                .await,
            Err(RoomError::ClosedOrExpired)
        ));
    }

    #[tokio::test]
    async fn signaling_disconnect_keeps_host_room_until_timeout() {
        let manager = RoomManager::with_presence_timeout(
            Duration::from_secs(60),
            Duration::from_secs(1),
            Duration::from_secs(1),
            "http://localhost:3100".to_owned(),
        );
        let created = manager.create_room("Host").await.unwrap();

        manager
            .signal_disconnected(created.room.id, created.participant.id)
            .await
            .unwrap();

        let room = manager
            .get_room(created.room.id, created.participant.id)
            .await
            .unwrap();
        assert_eq!(room.participant_count, 1);
        assert_eq!(
            room.participants[0].connection_state,
            rooms::ConnectionState::Disconnected
        );
    }
}
