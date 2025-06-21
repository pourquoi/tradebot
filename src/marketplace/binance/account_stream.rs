use std::{env, time::Duration};

use base64::{prelude::BASE64_STANDARD, Engine as _};
use chrono::Utc;
use ed25519_dalek::{
    ed25519::signature::SignerMut,
    pkcs8::DecodePrivateKey, SigningKey,
};
use futures::SinkExt;
use futures_util::StreamExt;
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::{fs, select, sync::broadcast::Sender, time::interval};
use tokio_tungstenite::connect_async;
use tracing::{debug, error, info};
use tungstenite::Message;

use crate::{
    marketplace::{
        binance::WS_ENDPOINT, MarketplaceAccountStream, MarketplaceEvent, MarketplaceOrderUpdate,
        MarketplacePortfolioUpdate,
    },
    order::{OrderStatus, OrderTrade},
    portfolio::Asset,
    AppEvent,
};

use super::Binance;

#[derive(Deserialize, Clone, Debug)]
struct AccountUpdateStream {
    #[serde(rename = "E")]
    pub event_time: u64,
    #[serde(rename = "u")]
    pub last_updated_at: u64,
    #[serde(rename = "B")]
    pub balances: Vec<AccountBalance>,
}

#[derive(Deserialize, Clone, Debug)]
struct AccountBalance {
    #[serde(rename = "a")]
    pub symbol: String,
    #[serde(rename = "f")]
    #[serde(with = "rust_decimal::serde::str")]
    pub free: Decimal,
    #[serde(rename = "l")]
    #[serde(with = "rust_decimal::serde::str")]
    pub locked: Decimal,
}

#[derive(Deserialize, Clone, Debug)]
struct OrderUpdate {
    #[serde(rename = "E")]
    pub time: u64,
    #[serde(rename = "s")]
    pub symbol: String,
    #[serde(rename = "i")]
    pub id: u64,
    #[serde(rename = "c")]
    pub client_id: String,

    #[serde(rename = "X")]
    pub status: String,
    #[serde(rename = "x")]
    pub execution_status: String,

    #[serde(rename = "S")]
    pub side: String,
    #[serde(rename = "o")]
    pub order_type: String,

    #[serde(rename = "n")]
    #[serde(with = "rust_decimal::serde::str")]
    pub commission_amount: Decimal,
    #[serde(rename = "N")]
    pub commission_asset: Option<String>,

    #[serde(rename = "q")]
    #[serde(with = "rust_decimal::serde::str")]
    pub amount: Decimal,
    #[serde(rename = "Q")]
    #[serde(with = "rust_decimal::serde::str")]
    pub quote_amount: Decimal,
    #[serde(rename = "p")]
    #[serde(with = "rust_decimal::serde::str")]
    pub price: Decimal,
    #[serde(rename = "P")]
    #[serde(with = "rust_decimal::serde::str")]
    pub stop_price: Decimal,

    #[serde(rename = "l")]
    #[serde(with = "rust_decimal::serde::str")]
    pub trade_amount: Decimal,
    #[serde(rename = "L")]
    #[serde(with = "rust_decimal::serde::str")]
    pub trade_price: Decimal,

    #[serde(rename = "T")]
    pub transaction_time: u64,
    #[serde(rename = "t")]
    pub trade_id: i64,
    
    #[serde(rename = "O")]
    pub creation_time: u64,
    #[serde(rename = "W")]
    pub working_time: Option<u64>,

    #[serde(rename = "Z")]
    #[serde(with = "rust_decimal::serde::str")]
    pub cumulative_quote_amount: Decimal,
    #[serde(rename = "z")]
    #[serde(with = "rust_decimal::serde::str")]
    pub cumulative_base_amount: Decimal,
}

impl TryFrom<&OrderUpdate> for MarketplaceOrderUpdate {
    type Error = String;

    fn try_from(value: &OrderUpdate) -> Result<Self, Self::Error> {
        let mut update = MarketplaceOrderUpdate {
            marketplace_id: value.id.to_string(),
            client_id: value.client_id.clone(),
            time: value.time,
            status: OrderStatus::try_from(value.status.clone())?,
            working_time: value.working_time,
            trade: None,
            update_type: value.execution_status.clone(),
        };

        if value.execution_status == "TRADE" {
            update.trade = Some(OrderTrade {
                id: value.trade_id.to_string(),
                trade_time: value.transaction_time,
                price: value.trade_price,
                amount: value.trade_amount,
            })
        }
        Ok(update)
    }
}

async fn logon(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    id: u64,
) -> anyhow::Result<()> {
    let api_key = env::var("BINANCE_SECURE_API_KEY")?;
    let private_key = env::var("BINANCE_PRIVATE_KEY")?;
    let private_key = fs::read_to_string(private_key).await?;

    let mut signing_key = SigningKey::from_pkcs8_pem(private_key.as_str())?;
    let timestamp = Utc::now().timestamp_millis();
    let payload = format!("apiKey={}&timestamp={}", api_key, timestamp);

    let signature = signing_key.sign(payload.as_bytes());
    let signature_b64 = BASE64_STANDARD.encode(signature.to_bytes());
    info!("signature for {} : {}", payload, signature_b64);

    let request = json!({
        "id": id,
        "method": "session.logon",
        "params": {
            "apiKey": api_key,
            "signature": signature_b64,
            "timestamp": timestamp,
        }
    });

    ws.send(Message::Text(request.to_string().into())).await?;

    Ok(())
}

async fn subscribe(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    id: u64,
) -> anyhow::Result<()> {
    let subscribe_msg = json!({
        "id": id,
        "method": "userDataStream.subscribe",
    });

    ws.send(Message::Text(subscribe_msg.to_string().into()))
        .await?;
    Ok(())
}

impl MarketplaceAccountStream for Binance {
    async fn start_account_stream(&mut self, tx_app: Sender<AppEvent>) -> anyhow::Result<()> {
        let request = (*WS_ENDPOINT).to_string();

        loop {
            let mut ws_stream;
            let response;

            loop {
                info!("Connecting to user stream {request}");
                match connect_async(request.clone()).await {
                    Ok(res) => {
                        ws_stream = res.0;
                        response = res.1;
                        break;
                    }
                    Err(err) => {
                        info!("Failed to connect to stream: {err}");
                        tokio::time::sleep(Duration::from_secs(10)).await;
                    }
                }
            }

            info!("Connected to user stream {request}");
            for (header, _value) in response.headers() {
                debug!("\t{header}");
            }

            let mut heartbeat = interval(Duration::from_secs(60));
            let mut req_id = 0_u64;
            let mut logon_request_id = 0;
            let mut subscribe_request_id = 0;

            let mut last_logon_attempt = Utc::now().timestamp_millis() as u64;
            logon(&mut ws_stream, req_id).await?;
            req_id += 1;

            loop {
                select! {
                    _ = heartbeat.tick() => {
                        let status_msg = json!({
                            "id": req_id,
                            "method": "session.status"
                        });
                        ws_stream.send(Message::Text(status_msg.to_string().into())).await?;
                        req_id += 1;
                    }
                    message = ws_stream.next() => {
                        match message {
                            Some(Ok(Message::Text(text))) => {
                                debug!("{:?}", text);
                                let now = Utc::now().timestamp_millis() as u64;
                                let parsed: Value = serde_json::from_str(&text)?;
                                if let Some(result) = parsed.get("result") {
                                    if let Some(Value::Number(res_id)) = parsed.get("id") {
                                        if res_id.as_u64() == Some(logon_request_id) {
                                            if let Some(Value::Number(_authorized_since)) = result.get("authorizedSince") {
                                                if let Some(Value::Bool(subscribed)) = result.get("userDataStream") {
                                                    if !subscribed {
                                                        subscribe_request_id = req_id;
                                                        subscribe(&mut ws_stream, req_id).await?;
                                                        req_id += 1;
                                                    }
                                                }
                                            }
                                        }
                                        else if res_id.as_u64() == Some(subscribe_request_id) {
                                            let account_msg = json!({
                                                "id": req_id,
                                                "method": "account.status",
                                                "params": {
                                                    "timestamp": now,
                                                    "omitZeroBalances": true
                                                }
                                            });
                                            ws_stream.send(Message::Text(account_msg.to_string().into())).await?;
                                            req_id += 1;
                                        }
                                        else if let Some(Value::Null) = result.get("authorizedSince") {
                                            if Duration::from_millis(now.saturating_sub(last_logon_attempt)) > Duration::from_secs(60) {
                                                logon_request_id = req_id;
                                                last_logon_attempt = now;
                                                logon(&mut ws_stream, req_id).await?;
                                                req_id += 1;
                                            } else {
                                                info!("Too early to login");
                                            }
                                        }
                                    }

                                } else if let Some(event) = parsed.get("event") {
                                    if let Some(Value::String(e)) = event.get("e") {
                                        match e.as_str() {
                                            "executionReport" => {
                                                match serde_json::from_value::<OrderUpdate>(event.clone()) {
                                                    Ok(update) => {
                                                        match MarketplaceOrderUpdate::try_from(&update) {
                                                            Ok(update) => {
                                                               let _ = tx_app.send(AppEvent::MarketPlace(MarketplaceEvent::OrderUpdate(update)));
                                                            }
                                                            Err(err) => {
                                                                error!("{err}");
                                                            }
                                                        }
                                                    }
                                                    Err(err) => {
                                                        error!("{err}");
                                                    }
                                                }
                                            },
                                            "outboundAccountPosition" => {
                                                match serde_json::from_value::<AccountUpdateStream>(event.clone()) {
                                                    Ok(update) => {
                                                        let _ = tx_app.send(AppEvent::MarketPlace(
                                                            MarketplaceEvent::PortfolioUpdate(MarketplacePortfolioUpdate {
                                                                time: update.event_time,
                                                                assets: update.balances.iter().map(|b| Asset {
                                                                    symbol: b.symbol.clone(),
                                                                    amount: b.free,
                                                                    locked: b.locked,
                                                                    value: None
                                                                }).collect()
                                                            })));
                                                    }
                                                    Err(err) => {
                                                        error!("{err}");
                                                    }
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                                else if parsed.get("error").is_some() {
                                    error!("Error response: {}", parsed);
                                }
                            }
                            Some(Ok(Message::Ping(data))) => {
                                debug!("Received ping: {:?}", data);
                                ws_stream.send(Message::Pong(data)).await.unwrap();
                            }
                            Some(Ok(Message::Close(frame))) => {
                                error!("Stream closed: {:?}", frame);
                                break;
                            }
                            Some(Ok(_)) => {}
                            Some(Err(err)) => {
                                error!("Stream error: {}", err);
                                break;

                            }
                            None => {}
                        }
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    }
}
