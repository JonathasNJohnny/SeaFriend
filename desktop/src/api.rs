use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct CreateRoomRequest {
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct JoinRoomRequest {
    pub code: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize)]
struct ParticipantBody {
    participant_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Participant {
    pub id: String,
    pub display_name: String,
    pub is_host: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Room {
    pub id: String,
    pub participant_count: usize,
    pub max_participants: usize,
    pub participants: Vec<Participant>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RoomResponse {
    pub room: Room,
    pub participant: Participant,
    pub access_code: Option<String>,
    pub invite_link: Option<String>,
}

#[derive(Clone)]
pub struct ApiClient {
    base_url: String,
    http: Client,
}

impl ApiClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            http: Client::new(),
        }
    }

    pub async fn create_room(&self, display_name: &str) -> Result<RoomResponse, String> {
        self.http
            .post(format!("{}/rooms", self.base_url))
            .json(&CreateRoomRequest {
                display_name: display_name.to_owned(),
            })
            .send()
            .await
            .map_err(|error| error.to_string())?
            .error_for_status()
            .map_err(|error| error.to_string())?
            .json::<RoomResponse>()
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn join_room(&self, code: &str, display_name: &str) -> Result<RoomResponse, String> {
        self.http
            .post(format!("{}/rooms/join", self.base_url))
            .json(&JoinRoomRequest {
                code: code.to_owned(),
                display_name: display_name.to_owned(),
            })
            .send()
            .await
            .map_err(|error| error.to_string())?
            .error_for_status()
            .map_err(|error| error.to_string())?
            .json::<RoomResponse>()
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn leave_room(&self, room_id: &str, participant_id: &str) -> Result<(), String> {
        self.http
            .post(format!("{}/rooms/{room_id}/leave", self.base_url))
            .json(&ParticipantBody {
                participant_id: participant_id.to_owned(),
            })
            .send()
            .await
            .map_err(|error| error.to_string())?
            .error_for_status()
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    pub async fn close_room(&self, room_id: &str, participant_id: &str) -> Result<(), String> {
        self.http
            .post(format!("{}/rooms/{room_id}/close", self.base_url))
            .json(&ParticipantBody {
                participant_id: participant_id.to_owned(),
            })
            .send()
            .await
            .map_err(|error| error.to_string())?
            .error_for_status()
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}
