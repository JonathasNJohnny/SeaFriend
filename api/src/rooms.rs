use rand::distr::{Alphanumeric, SampleString};
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

const MAX_PARTICIPANTS: usize = 10;
const MIN_DISPLAY_NAME_LENGTH: usize = 1;
const MAX_DISPLAY_NAME_LENGTH: usize = 64;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RoomStatus {
    Active,
    Closed,
}

#[derive(Debug, Clone, Serialize)]
pub struct ParticipantSnapshot {
    pub id: Uuid,
    pub display_name: String,
    pub joined_at: u64,
    pub last_seen_at: u64,
    pub connection_state: ConnectionState,
    pub is_host: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    Connected,
    Disconnected,
}

#[derive(Debug, Clone, Serialize)]
pub struct RoomSnapshot {
    pub id: Uuid,
    pub status: RoomStatus,
    pub created_at: u64,
    pub expires_at: u64,
    pub host_participant_id: Uuid,
    pub max_participants: usize,
    pub participant_count: usize,
    pub participants: Vec<ParticipantSnapshot>,
}

#[derive(Debug)]
struct Participant {
    id: Uuid,
    display_name: String,
    joined_at: u64,
    last_seen_at: u64,
    connection_state: ConnectionState,
}

#[derive(Debug)]
struct Room {
    id: Uuid,
    access_code: String,
    invite_token: String,
    created_at: u64,
    expires_at: u64,
    status: RoomStatus,
    host_participant_id: Uuid,
    participants: HashMap<Uuid, Participant>,
}

#[derive(Debug)]
struct RoomStore {
    rooms: HashMap<Uuid, Room>,
    by_access_code: HashMap<String, Uuid>,
    by_invite_token: HashMap<String, Uuid>,
}

#[derive(Debug)]
pub struct RoomManager {
    store: Mutex<RoomStore>,
    expiration: Duration,
    cleanup_interval: Duration,
    presence_timeout: Duration,
    invite_base_url: String,
}

#[derive(Debug)]
pub struct CreateRoomResult {
    pub room: RoomSnapshot,
    pub participant: ParticipantSnapshot,
    pub access_code: String,
    pub invite_link: String,
}

#[derive(Debug)]
pub struct JoinRoomResult {
    pub room: RoomSnapshot,
    pub participant: ParticipantSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomError {
    InvalidDisplayName,
    InvalidCodeOrInvite,
    NotFound,
    ClosedOrExpired,
    Full,
    ParticipantNotFound,
    Unauthorized,
    Forbidden,
}

impl RoomManager {
    pub fn new(expiration: Duration, cleanup_interval: Duration, invite_base_url: String) -> Self {
        Self::with_presence_timeout(
            expiration,
            cleanup_interval,
            Duration::from_secs(60),
            invite_base_url,
        )
    }

    pub fn with_presence_timeout(
        expiration: Duration,
        cleanup_interval: Duration,
        presence_timeout: Duration,
        invite_base_url: String,
    ) -> Self {
        Self {
            store: Mutex::new(RoomStore {
                rooms: HashMap::new(),
                by_access_code: HashMap::new(),
                by_invite_token: HashMap::new(),
            }),
            expiration,
            cleanup_interval,
            presence_timeout,
            invite_base_url: invite_base_url.trim_end_matches('/').to_owned(),
        }
    }

    pub fn cleanup_interval(&self) -> Duration {
        self.cleanup_interval
    }

    pub async fn create_room(&self, display_name: &str) -> Result<CreateRoomResult, RoomError> {
        let display_name = validate_display_name(display_name)?;
        let now = unix_now();
        let room_id = Uuid::new_v4();
        let participant_id = Uuid::new_v4();
        let access_code = self.unique_code();
        let invite_token = self.unique_invite_token();
        let participant = Participant {
            id: participant_id,
            display_name,
            joined_at: now,
            last_seen_at: now,
            connection_state: ConnectionState::Connected,
        };
        let room = Room {
            id: room_id,
            access_code: access_code.clone(),
            invite_token: invite_token.clone(),
            created_at: now,
            expires_at: now.saturating_add(self.expiration.as_secs()),
            status: RoomStatus::Active,
            host_participant_id: participant_id,
            participants: HashMap::from([(participant_id, participant)]),
        };

        let mut store = self.store.lock().expect("room store lock poisoned");
        store.by_access_code.insert(access_code.clone(), room_id);
        store.by_invite_token.insert(invite_token.clone(), room_id);
        let snapshot = room_snapshot(&room);
        let participant_snapshot = participant_snapshot(
            room.participants.get(&participant_id).expect("host exists"),
            participant_id,
        );
        store.rooms.insert(room_id, room);
        Ok(CreateRoomResult {
            room: snapshot,
            participant: participant_snapshot,
            access_code,
            invite_link: format!("{}/join/{}", self.invite_base_url, invite_token),
        })
    }

    pub async fn join_by_code(
        &self,
        code: &str,
        display_name: &str,
    ) -> Result<JoinRoomResult, RoomError> {
        self.join_with_lookup(code, display_name, |store, value| {
            store.by_access_code.get(value).copied()
        })
        .await
    }

    pub async fn join_by_invite(
        &self,
        token: &str,
        display_name: &str,
    ) -> Result<JoinRoomResult, RoomError> {
        self.join_with_lookup(token, display_name, |store, value| {
            store.by_invite_token.get(value).copied()
        })
        .await
    }

    async fn join_with_lookup<F>(
        &self,
        value: &str,
        display_name: &str,
        lookup: F,
    ) -> Result<JoinRoomResult, RoomError>
    where
        F: Fn(&RoomStore, &str) -> Option<Uuid>,
    {
        let display_name = validate_display_name(display_name)?;
        let now = unix_now();
        let mut store = self.store.lock().expect("room store lock poisoned");
        let room_id = lookup(&store, value).ok_or(RoomError::InvalidCodeOrInvite)?;
        let room = store.rooms.get_mut(&room_id).ok_or(RoomError::NotFound)?;
        ensure_active(room, now)?;
        if room.participants.len() >= MAX_PARTICIPANTS {
            return Err(RoomError::Full);
        }

        let participant_id = Uuid::new_v4();
        room.participants.insert(
            participant_id,
            Participant {
                id: participant_id,
                display_name,
                joined_at: now,
                last_seen_at: now,
                connection_state: ConnectionState::Connected,
            },
        );
        let participant = participant_snapshot(
            room.participants
                .get(&participant_id)
                .expect("participant inserted"),
            room.host_participant_id,
        );
        Ok(JoinRoomResult {
            room: room_snapshot(room),
            participant,
        })
    }

    pub async fn get_room(
        &self,
        room_id: Uuid,
        participant_id: Uuid,
    ) -> Result<RoomSnapshot, RoomError> {
        let mut store = self.store.lock().expect("room store lock poisoned");
        let room = store.rooms.get_mut(&room_id).ok_or(RoomError::NotFound)?;
        ensure_active(room, unix_now())?;
        authorize(room, participant_id)?;
        Ok(room_snapshot(room))
    }

    pub async fn get_participants(
        &self,
        room_id: Uuid,
        participant_id: Uuid,
    ) -> Result<Vec<ParticipantSnapshot>, RoomError> {
        Ok(self.get_room(room_id, participant_id).await?.participants)
    }

    pub async fn heartbeat(&self, room_id: Uuid, participant_id: Uuid) -> Result<(), RoomError> {
        let mut store = self.store.lock().expect("room store lock poisoned");
        let room = store.rooms.get_mut(&room_id).ok_or(RoomError::NotFound)?;
        ensure_active(room, unix_now())?;
        let participant = room
            .participants
            .get_mut(&participant_id)
            .ok_or(RoomError::Unauthorized)?;
        participant.last_seen_at = unix_now();
        participant.connection_state = ConnectionState::Connected;
        Ok(())
    }

    pub async fn leave(&self, room_id: Uuid, participant_id: Uuid) -> Result<(), RoomError> {
        let mut store = self.store.lock().expect("room store lock poisoned");
        let room = store.rooms.get_mut(&room_id).ok_or(RoomError::NotFound)?;
        ensure_active(room, unix_now())?;
        if room.host_participant_id == participant_id {
            close_room(room);
            return Ok(());
        }
        room.participants
            .remove(&participant_id)
            .map(|_| ())
            .ok_or(RoomError::ParticipantNotFound)
    }

    pub async fn disconnect(&self, room_id: Uuid, participant_id: Uuid) -> Result<(), RoomError> {
        let mut store = self.store.lock().expect("room store lock poisoned");
        let room = store.rooms.get_mut(&room_id).ok_or(RoomError::NotFound)?;
        ensure_active(room, unix_now())?;
        if room.host_participant_id == participant_id {
            close_room(room);
            return Ok(());
        }
        let participant = room
            .participants
            .get_mut(&participant_id)
            .ok_or(RoomError::ParticipantNotFound)?;
        participant.connection_state = ConnectionState::Disconnected;
        Ok(())
    }

    pub async fn signal_disconnected(
        &self,
        room_id: Uuid,
        participant_id: Uuid,
    ) -> Result<(), RoomError> {
        let mut store = self.store.lock().expect("room store lock poisoned");
        let room = store.rooms.get_mut(&room_id).ok_or(RoomError::NotFound)?;
        ensure_active(room, unix_now())?;
        let participant = room
            .participants
            .get_mut(&participant_id)
            .ok_or(RoomError::ParticipantNotFound)?;
        participant.connection_state = ConnectionState::Disconnected;
        participant.last_seen_at = unix_now();
        Ok(())
    }

    pub async fn close(&self, room_id: Uuid, participant_id: Uuid) -> Result<(), RoomError> {
        let mut store = self.store.lock().expect("room store lock poisoned");
        let room = store.rooms.get_mut(&room_id).ok_or(RoomError::NotFound)?;
        ensure_active(room, unix_now())?;
        authorize(room, participant_id)?;
        if room.host_participant_id != participant_id {
            return Err(RoomError::Forbidden);
        }
        close_room(room);
        Ok(())
    }

    pub async fn cleanup_expired(&self) {
        let now = unix_now();
        let mut store = self.store.lock().expect("room store lock poisoned");
        let expired: Vec<Uuid> = store
            .rooms
            .iter()
            .filter_map(|(id, room)| (room.expires_at <= now).then_some(*id))
            .collect();
        for room_id in expired {
            remove_room(&mut store, room_id);
        }

        let stale_cutoff = now.saturating_sub(self.presence_timeout.as_secs());
        let room_ids: Vec<Uuid> = store.rooms.keys().copied().collect();
        for room_id in room_ids {
            let Some(room) = store.rooms.get_mut(&room_id) else {
                continue;
            };
            if room.status == RoomStatus::Closed {
                continue;
            }

            let stale_ids: Vec<Uuid> = room
                .participants
                .values()
                .filter(|participant| participant.last_seen_at <= stale_cutoff)
                .map(|participant| participant.id)
                .collect();
            if stale_ids.contains(&room.host_participant_id) {
                close_room(room);
                continue;
            }
            for participant_id in stale_ids {
                room.participants.remove(&participant_id);
            }
        }
    }

    fn unique_code(&self) -> String {
        let mut rng = rand::rng();
        loop {
            let code: String = Alphanumeric.sample_string(&mut rng, 26);
            let store = self.store.lock().expect("room store lock poisoned");
            if !store.by_access_code.contains_key(&code) {
                return code;
            }
        }
    }

    fn unique_invite_token(&self) -> String {
        let mut rng = rand::rng();
        loop {
            let token: String = Alphanumeric.sample_string(&mut rng, 43);
            let store = self.store.lock().expect("room store lock poisoned");
            if !store.by_invite_token.contains_key(&token) {
                return token;
            }
        }
    }
}

fn validate_display_name(value: &str) -> Result<String, RoomError> {
    let trimmed = value.trim();
    let length = trimmed.chars().count();
    if !(MIN_DISPLAY_NAME_LENGTH..=MAX_DISPLAY_NAME_LENGTH).contains(&length) {
        return Err(RoomError::InvalidDisplayName);
    }
    Ok(trimmed.to_owned())
}

fn ensure_active(room: &mut Room, now: u64) -> Result<(), RoomError> {
    if room.status == RoomStatus::Closed {
        return Err(RoomError::ClosedOrExpired);
    }
    if room.expires_at <= now {
        close_room(room);
        return Err(RoomError::ClosedOrExpired);
    }
    Ok(())
}

fn authorize(room: &Room, participant_id: Uuid) -> Result<(), RoomError> {
    if room.participants.contains_key(&participant_id) {
        Ok(())
    } else {
        Err(RoomError::Unauthorized)
    }
}

fn close_room(room: &mut Room) {
    room.status = RoomStatus::Closed;
    room.participants.clear();
}

fn remove_room(store: &mut RoomStore, room_id: Uuid) {
    if let Some(room) = store.rooms.remove(&room_id) {
        store.by_access_code.remove(&room.access_code);
        store.by_invite_token.remove(&room.invite_token);
    }
}

fn room_snapshot(room: &Room) -> RoomSnapshot {
    let mut participants: Vec<_> = room
        .participants
        .values()
        .map(|participant| participant_snapshot(participant, room.host_participant_id))
        .collect();
    participants.sort_by_key(|participant| participant.joined_at);
    RoomSnapshot {
        id: room.id,
        status: room.status.clone(),
        created_at: room.created_at,
        expires_at: room.expires_at,
        host_participant_id: room.host_participant_id,
        max_participants: MAX_PARTICIPANTS,
        participant_count: participants.len(),
        participants,
    }
}

fn participant_snapshot(participant: &Participant, host_id: Uuid) -> ParticipantSnapshot {
    ParticipantSnapshot {
        id: participant.id,
        display_name: participant.display_name.clone(),
        joined_at: participant.joined_at,
        last_seen_at: participant.last_seen_at,
        connection_state: participant.connection_state.clone(),
        is_host: participant.id == host_id,
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before Unix epoch")
        .as_secs()
}
