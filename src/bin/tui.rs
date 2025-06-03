use std::time::Duration;

use clap::Parser;
use futures::StreamExt;
use tokio::select;
use tokio_tungstenite::connect_async;
use tracing::info;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use tungstenite::Message;

use trading_bot::{tui::app::App, *};

#[derive(Parser, Debug)]
struct Args {
    #[arg(long, default_value = "127.0.0.1:5555")]
    address: String,
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let args = Args::parse();

    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            format!(
                "{}=debug,tower_http=debug,reqwest=debug",
                env!("CARGO_CRATE_NAME")
            )
            .into()
        }))
        .with(fmt::layer())
        .init();

    let (tx, rx) = tokio::sync::mpsc::channel::<AppEvent>(100);
    let mut app = App::new(rx);

    let ws_client_task = tokio::task::spawn(async move {
        let mut stream;
        loop {
            let url = format!("ws://{}/ws", args.address);
            //info!("Connecting to stream {url}");
            if let Ok(res) = connect_async(url).await {
                stream = res.0;
                break;
            }
            tokio::time::sleep(Duration::from_millis(1000)).await;
        }

        let tx = tx.clone();

        while let Some(Ok(msg)) = stream.next().await {
            match msg {
                Message::Text(msg) => {
                    if let Ok(event) = serde_json::de::from_slice::<AppEvent>(msg.as_bytes()) {
                        let _ = tx.send(event).await;
                    }
                }
                Message::Close(frame) => {
                    info!("Connection closed : {:?}", frame);
                    break;
                }
                _ => {}
            };
        }
    });

    let app_task = tokio::task::spawn(async move {
        let _ = app.run().await;
    });

    select! {
        _ = app_task => {},
        _ = ws_client_task => {}
    }

    ratatui::restore();
}
