use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use tokio::sync::broadcast;
use tower_http::services::ServeDir;

use crate::game::SharedGame;

#[derive(Clone)]
pub struct AppState {
    pub game: SharedGame,
    pub tick_tx: broadcast::Sender<String>,
}

pub fn create_router(state: AppState, static_dir: &str) -> Router {
    Router::new()
        .route("/ws", get(ws_handler))
        .route("/api/state", get(get_state))
        .fallback_service(ServeDir::new(static_dir))
        .with_state(state)
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state))
}

async fn handle_ws(mut socket: WebSocket, state: AppState) {
    // Send full state on connect
    let init = {
        let g = state.game.read().await;
        let snapshot = g.snapshot();
        serde_json::json!({
            "type": "init",
            "state": snapshot,
        })
    };

    if socket.send(Message::Text(init.to_string().into())).await.is_err() {
        return;
    }

    let mut tick_rx = state.tick_tx.subscribe();

    loop {
        tokio::select! {
            result = tick_rx.recv() => {
                match result {
                    Ok(tick_json) => {
                        let msg = serde_json::json!({
                            "type": "tick",
                            "state": serde_json::from_str::<serde_json::Value>(&tick_json).unwrap_or_default(),
                        });
                        if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(input) = serde_json::from_str::<ClientInput>(&text) {
                            match input {
                                ClientInput::Move { direction } => {
                                    let mut g = state.game.write().await;
                                    g.set_move_intent(&direction);
                                    if let Err(e) = g.tick() {
                                        eprintln!("Tick error: {}", e);
                                        continue;
                                    }
                                    let snapshot = g.snapshot();
                                    let json = serde_json::to_string(&snapshot).unwrap_or_default();
                                    let _ = state.tick_tx.send(json);
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ClientInput {
    #[serde(rename = "move")]
    Move { direction: String },
}

async fn get_state(State(state): State<AppState>) -> impl IntoResponse {
    let g = state.game.read().await;
    axum::Json(g.snapshot())
}
