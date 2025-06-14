use std::{
    net::SocketAddr,
    sync::{mpsc::Receiver, Arc},
};

use anyhow::Result;
use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::IntoResponse,
    routing::any,
    Router,
};
use futures_util::{sink::SinkExt, stream::StreamExt};
use tokio::{
    select,
    sync::{broadcast, mpsc, RwLock},
};
use tower_http::trace::TraceLayer;
use tracing::{error, info, trace};

use crate::{state::StateEvent, AppCommandEvent, AppEvent};

struct ServerState {
    app_tx: broadcast::Sender<AppEvent>,
    cmd_tx: mpsc::Sender<AppCommandEvent>,
    app_state: Arc<RwLock<crate::state::State>>,
}

type SharedServerState = Arc<ServerState>;

pub async fn start(
    address: String,
    app_state: Arc<RwLock<crate::state::State>>,
    app_tx: broadcast::Sender<AppEvent>,
    cmd_tx: mpsc::Sender<AppCommandEvent>,
) -> Result<()> {
    let state = Arc::from(ServerState {
        app_tx,
        cmd_tx,
        app_state,
    });

    let app = Router::new()
        .route("/ws", any(ws_handler))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(address).await?;

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;

    Ok(())
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<SharedServerState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: SharedServerState) {
    let mut app_rx = state.app_tx.subscribe();
    let (mut sender, mut receiver) = socket.split();

    // send portfolio and orders
    {
        let app_state = state.app_state.read().await;

        let event = AppEvent::State(StateEvent::Portfolio(app_state.portfolio.clone()));
        let msg = serde_json::ser::to_string(&event).unwrap();
        let _ = sender
            .send(axum::extract::ws::Message::Text(msg.into()))
            .await;

        let event = AppEvent::State(StateEvent::Orders(app_state.orders.clone()));
        let msg = serde_json::ser::to_string(&event).unwrap();
        let _ = sender
            .send(axum::extract::ws::Message::Text(msg.into()))
            .await;
    }

    let mut send_task = tokio::task::spawn(async move {
        loop {
            if let Ok(event) = app_rx.recv().await {
                let msg = serde_json::ser::to_string(&event).unwrap();
                if sender
                    .send(axum::extract::ws::Message::Text(msg.into()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
        }
    });

    let mut recv_task = tokio::task::spawn(async move {
        let cmd_tx = state.cmd_tx.clone();
        while let Some(Ok(msg)) = receiver.next().await {
            info!("Received websocket message : {:?}", msg);
            if let Message::Text(msg) = msg {
                if let Ok(cmd) = serde_json::de::from_str::<AppCommandEvent>(msg.as_str()) {
                    let _ = cmd_tx.send(cmd).await;
                }
            }
        }
    });

    select! {
        result = (&mut send_task) => {
            recv_task.abort();
        }
        result = (&mut recv_task) => {
            send_task.abort();
        }
    }
}
