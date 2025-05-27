use std::time::Duration;

use futures::SinkExt;
use futures_util::StreamExt;
use rust_decimal::Decimal;
use serde::Deserialize;
use tokio::sync::broadcast::Sender;
use tokio_tungstenite::connect_async;
use tracing::debug;
use tracing::error;
use tracing::info;
use tungstenite::Message;

use crate::ticker::Ticker;

use super::Binance;

#[derive(Deserialize, Clone, Debug)]
#[allow(dead_code)]
pub struct MultiStream<T> {
    pub stream: String,
    pub data: T,
}

#[derive(Deserialize, Clone, Debug)]
#[allow(dead_code)]
pub struct MarkPriceStream {
    #[serde(rename = "e")]
    pub event_type: String,

    #[serde(rename = "E")]
    pub event_time: u64,

    #[serde(rename = "s")]
    pub symbol: String,

    #[serde(rename = "p")]
    #[serde(with = "rust_decimal::serde::str")]
    pub mark_price: Decimal,

    #[serde(rename = "i")]
    #[serde(with = "rust_decimal::serde::str")]
    pub index_price: Decimal,
}

#[derive(Deserialize, Clone, Debug)]
#[allow(dead_code)]
pub struct KLineStream {
    #[serde(rename = "e")]
    pub event_type: String,

    #[serde(rename = "E")]
    pub event_time: u64,

    #[serde(rename = "s")]
    pub symbol: String,

    #[serde(rename = "k")]
    pub data: KLineData,
}

#[derive(Deserialize, Clone, Debug)]
#[allow(dead_code)]
pub struct KLineData {
    #[serde(rename = "h")]
    #[serde(with = "rust_decimal::serde::str")]
    pub high_price: Decimal,

    #[serde(rename = "l")]
    #[serde(with = "rust_decimal::serde::str")]
    pub low_price: Decimal,

    #[serde(rename = "n")]
    pub trade_count: u64,

    #[serde(rename = "t")]
    pub start_time: u64,

    #[serde(rename = "T")]
    pub close_time: u64,
}

#[derive(Deserialize, Debug, Clone)]
#[allow(dead_code)]
pub struct TradeStream {
    #[serde(rename = "e")]
    pub event_type: String,

    #[serde(rename = "E")]
    pub event_time: u64,

    #[serde(rename = "s")]
    pub symbol: String,

    #[serde(rename = "t")]
    pub trade_id: u64,

    #[serde(rename = "T")]
    pub trade_time: u64,

    #[serde(rename = "p")]
    #[serde(with = "rust_decimal::serde::str")]
    pub price: Decimal,

    #[serde(rename = "q")]
    #[serde(with = "rust_decimal::serde::str")]
    pub quantity: Decimal,

    #[serde(rename = "m")]
    pub maker_maker: bool,
}

impl Binance {
    pub async fn listen_trade_stream(
        &self,
        tickers: Vec<Ticker>,
        tx: Sender<crate::marketplace::MarketPlaceEvent>,
    ) {
        let params = tickers
            .iter()
            //.map(|s| format!("{}@kline_1m", s))
            .map(|s| format!("{}{}@trade", s.base.to_lowercase(), s.quote.to_lowercase()))
            .collect::<Vec<String>>()
            .join("/");

        let request = format!("wss://stream.binance.com/stream?streams={}", params);
        info!("{request}");

        loop {
            tokio::time::sleep(Duration::from_secs(3)).await;

            let mut ws_stream;
            let response;

            loop {
                match connect_async(request.clone()).await {
                    Ok(res) => {
                        ws_stream = res.0;
                        response = res.1;
                        break;
                    }
                    Err(err) => {
                        info!("Failed to connect to stream: {:?}", err);
                        tokio::time::sleep(Duration::from_secs(10)).await;
                    }
                }
            }

            info!("Connected to websocket");
            for (header, _value) in response.headers() {
                debug!("\t{header}");
            }

            loop {
                match tokio::time::timeout(Duration::from_secs(60), ws_stream.next()).await {
                    Ok(Some(message)) => match message {
                        Ok(Message::Text(message)) => {
                            match serde_json::de::from_slice::<MultiStream<TradeStream>>(
                                message.as_ref(),
                            ) {
                                Ok(stream) => match Ticker::try_from(stream.data.symbol.as_str()) {
                                    Ok(ticker) => {
                                        let _ =
                                            tx.send(crate::marketplace::MarketPlaceEvent::Trade(
                                                crate::marketplace::TradeEvent {
                                                    ticker,
                                                    price: stream.data.price,
                                                    quantity: stream.data.quantity,
                                                    trade_id: stream.data.trade_id,
                                                    trade_time: stream.data.trade_time,
                                                },
                                            ));
                                    }
                                    Err(_) => {
                                        error!("Invalid symbol {}", stream.data.symbol);
                                    }
                                },
                                Err(err) => {
                                    error!("Stream error: {}", err);
                                }
                            }
                        }
                        Ok(Message::Ping(data)) => {
                            debug!("Received ping: {:?}", data);
                            ws_stream.send(Message::Pong(data)).await.unwrap();
                        }
                        Ok(Message::Close(frame)) => {
                            error!("Stream closed: {:?}", frame);
                            break;
                        }
                        Ok(_) => {}
                        Err(err) => {
                            error!("Stream error: {}", err);
                            break;
                        }
                    },
                    Ok(None) => {
                        error!("Empty stream");
                        break;
                    }
                    Err(err) => {
                        error!("Stream error: {}", err);
                        break;
                    }
                }
            }
            info!("Reconnecting");
        }
    }
}
