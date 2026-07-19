//! Realtime WebSocket gateway (interfaces/realtime-protocol.md §Connection
//! lifecycle). Validates the ticket handshake, then bridges the socket to the
//! game loop: inbound frames → `ServerEvent::Client`, and a per-connection
//! outbound channel the loop writes authoritative messages to.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use meld_api::Tickets;
use meld_db::Db;
use meld_proto::realtime::session as ws;
use meld_proto::realtime::Message as _;
use meld_proto::{Envelope, RawEnvelope};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::game::{GameHandle, ServerEvent};

#[derive(Clone)]
pub struct GatewayState {
    pub db: Db,
    pub tickets: Tickets,
    pub game: GameHandle,
    pub heartbeat_interval_ms: i32,
    pub grace_window_ms: i32,
    pub auth_timeout_ms: u64,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// `GET /v1/realtime` — upgrade to WebSocket.
pub async fn realtime_handler(ws: WebSocketUpgrade, State(gw): State<GatewayState>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, gw))
}

fn envelope_json<M: meld_proto::realtime::Message>(seq: u32, payload: &M) -> String {
    let env = Envelope::new(M::TYPE, seq, now_ms(), payload);
    serde_json::to_string(&env).expect("envelope serializes")
}

async fn handle_socket(socket: WebSocket, gw: GatewayState) {
    let (mut sink, mut stream) = socket.split();

    // --- auth handshake: first frame must be session.authenticate ----------
    let first =
        tokio::time::timeout(Duration::from_millis(gw.auth_timeout_ms), stream.next()).await;
    let text = match first {
        Ok(Some(Ok(Message::Text(t)))) => t,
        _ => {
            let _ = sink
                .send(Message::Text(envelope_json(
                    1,
                    &ws::Error {
                        code: meld_proto::enums::ErrorCode::Unauthorized,
                        message: "Expected session.authenticate.".into(),
                        client_seq: None,
                    },
                )))
                .await;
            return;
        }
    };

    let raw: RawEnvelope = match serde_json::from_str::<RawEnvelope>(&text) {
        Ok(r) if r.msg_type == ws::Authenticate::TYPE => r,
        _ => {
            let _ = sink
                .send(Message::Text(unauthorized(
                    "First message must be session.authenticate.",
                )))
                .await;
            return;
        }
    };
    let auth: ws::Authenticate = match serde_json::from_value(raw.payload) {
        Ok(a) => a,
        Err(_) => {
            let _ = sink
                .send(Message::Text(unauthorized(
                    "Malformed authenticate payload.",
                )))
                .await;
            return;
        }
    };

    // Consume the single-use ticket.
    let Some(player_id) = gw.tickets.consume(&auth.ticket) else {
        let _ = sink
            .send(Message::Text(unauthorized("Invalid or expired ticket.")))
            .await;
        return;
    };

    let username = match gw.db.get_player(player_id).await {
        Ok(Some(p)) => p.username,
        _ => {
            let _ = sink
                .send(Message::Text(unauthorized("Account not found.")))
                .await;
            return;
        }
    };

    let player_id = player_id.to_string();
    let session_id = Uuid::now_v7().to_string();

    // Handshake success — seq 1 (fresh session; counters reset).
    let authed = ws::Authenticated {
        client_seq: raw.seq,
        session_id: session_id.clone(),
        player_id: player_id.clone(),
        resumed: false,
        heartbeat_interval_ms: gw.heartbeat_interval_ms,
        grace_window_ms: gw.grace_window_ms,
        server_ts: now_ms(),
        last_client_seq: 0,
    };
    if sink
        .send(Message::Text(envelope_json(1, &authed)))
        .await
        .is_err()
    {
        return;
    }

    // --- steady state ------------------------------------------------------
    // Bounded so a slow client's queue can't grow without limit; the game loop
    // `try_send`s and drops a client that overflows (see game.rs `dispatch`).
    let (out_tx, mut out_rx) = mpsc::channel::<String>(crate::game::OUT_CHANNEL_CAP);

    // Writer task: forward loop output to the socket.
    let mut writer = tokio::spawn(async move {
        while let Some(text) = out_rx.recv().await {
            if sink.send(Message::Text(text)).await.is_err() {
                break;
            }
        }
    });

    gw.game
        .send(ServerEvent::Connected {
            player_id: player_id.clone(),
            username,
            session_id,
            out: out_tx,
        })
        .await;

    // Reader loop: parse frames and forward to the game loop.
    let reader = async {
        while let Some(msg) = stream.next().await {
            match msg {
                Ok(Message::Text(t)) => {
                    if let Ok(raw) = serde_json::from_str::<RawEnvelope>(&t) {
                        gw.game
                            .send(ServerEvent::Client {
                                player_id: player_id.clone(),
                                raw,
                            })
                            .await;
                    }
                }
                Ok(Message::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
    };

    tokio::select! {
        _ = reader => {},
        _ = &mut writer => {},
    }
    writer.abort();

    gw.game.send(ServerEvent::Disconnected { player_id }).await;
}

fn unauthorized(msg: &str) -> String {
    envelope_json(
        1,
        &ws::Error {
            code: meld_proto::enums::ErrorCode::Unauthorized,
            message: msg.to_string(),
            client_seq: None,
        },
    )
}
