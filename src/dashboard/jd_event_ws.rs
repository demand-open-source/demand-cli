use axum::{
    extract::{
        ws::{WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
};
use futures::StreamExt;
use serde::Serialize;
use tokio::sync::watch;
use tokio_stream::wrappers::WatchStream;
use tracing::warn;

use crate::api::AppState;

#[derive(Debug, Serialize, Clone)]
pub struct NewTemplateNotification {
    pub event: String,
    pub message: String,
    pub template_id: u64,
    pub timestamp: u64,
}
impl NewTemplateNotification {
    pub fn new(event: String, message: String, template_id: u64, timestamp: u64) -> Self {
        Self {
            event,
            message,
            template_id,
            timestamp,
        }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct JobDeclarationData {
    pub template_id: Option<u64>,
    pub channel_id: Option<u32>,
    pub req_id: Option<u32>,
    pub job_id: Option<u32>,
    pub mining_job_token: Option<String>, // hex encoded mining job token
}

impl JobDeclarationData {
    pub fn new() -> Self {
        Self {
            template_id: None,
            channel_id: None,
            req_id: None,
            job_id: None,
            mining_job_token: None,
        }
    }
}

pub type TemplateNotificationBroadcaster = watch::Sender<Option<NewTemplateNotification>>;

/// Streams template event notifications to a WebSocket client as JSON messages.
///
/// Listens for retained notifications and sends them to the connected WebSocket client.
/// Serializes each notification to JSON and sends it as a text message. If sending fails,
/// the loop breaks and the connection is closed.
///
/// # Arguments
/// * `socket` - The WebSocket connection to send messages to.
/// * `subscriber` - The retained notification receiver.
async fn stream_event_notifications(
    mut socket: WebSocket,
    subscriber: watch::Receiver<Option<NewTemplateNotification>>,
) {
    let mut stream = WatchStream::new(subscriber);

    while let Some(notification) = stream.next().await {
        if let Some(notification) = notification {
            if let Ok(json) = serde_json::to_string(&notification) {
                if socket
                    .send(axum::extract::ws::Message::Text(json.into()))
                    .await
                    .is_err()
                {
                    warn!("WebSocket closed while sending template notification");
                    break;
                }
            }
        }
    }

    warn!("Template WebSocket connection closed");
}

/// WebSocket handler for template event notifications.
///
/// Upgrades the HTTP connection to a WebSocket and streams template notifications to the client.
///
/// # Arguments
/// * `ws` - The WebSocket upgrade request.
/// * `State(state)` - The application state containing the template notification broadcaster.
pub async fn ws_event_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| {
        let jd_event_subscriber = state.jd_event_broadcaster.subscribe();

        stream_event_notifications(socket, jd_event_subscriber)
    })
}
