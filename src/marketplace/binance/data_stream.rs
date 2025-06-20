use std::time::Duration;

use chrono::Utc;
use futures::SinkExt;
use futures_util::StreamExt;
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::broadcast::Sender;
use tokio_tungstenite::connect_async;
use tracing::debug;
use tracing::error;
use tracing::info;
use tungstenite::Message;

use crate::marketplace::binance::STREAM_ENDPOINT;
use crate::marketplace::MarketplaceBook;
use crate::marketplace::MarketplaceCandle;
use crate::marketplace::MarketplaceEvent;
use crate::marketplace::MarketplaceTrade;
use crate::ticker::Ticker;
use crate::AppEvent;

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
    #[serde(rename = "o")]
    #[serde(with = "rust_decimal::serde::str")]
    pub open_price: Decimal,

    #[serde(rename = "c")]
    #[serde(with = "rust_decimal::serde::str")]
    pub close_price: Decimal,

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

    #[serde(rename = "q")]
    #[serde(with = "rust_decimal::serde::str")]
    pub volume: Decimal,

    #[serde(rename = "x")]
    pub closed: bool,
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

#[derive(Deserialize, Debug, Clone)]
#[allow(dead_code)]
pub struct PartialDepthStream {
    #[serde(rename = "lastUpdateId")]
    last_update_id: u64,

    #[serde(
        deserialize_with = "crate::utils::deserialize_decimal_pairs",
        serialize_with = "crate::utils::serialize_decimal_pairs"
    )]
    bids: Vec<(Decimal, Decimal)>,

    #[serde(
        deserialize_with = "crate::utils::deserialize_decimal_pairs",
        serialize_with = "crate::utils::serialize_decimal_pairs"
    )]
    asks: Vec<(Decimal, Decimal)>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct DepthUpdateStream {
    #[serde(rename = "e")]
    pub event_type: String,

    #[serde(rename = "E")]
    pub event_time: u64,

    #[serde(rename = "s")]
    pub symbol: String,

    #[serde(rename = "U")]
    pub first_update_id: u64,

    #[serde(rename = "u")]
    pub final_update_id: u64,

    #[serde(
        rename = "b",
        deserialize_with = "crate::utils::deserialize_decimal_pairs",
        serialize_with = "crate::utils::serialize_decimal_pairs"
    )]
    pub bids: Vec<(Decimal, Decimal)>,

    #[serde(
        rename = "a",
        deserialize_with = "crate::utils::deserialize_decimal_pairs",
        serialize_with = "crate::utils::serialize_decimal_pairs"
    )]
    pub asks: Vec<(Decimal, Decimal)>,
}

impl Binance {
    pub async fn connect_stream(&self, tickers: &Vec<Ticker>, tx: Sender<AppEvent>) {
        let trade_params = tickers
            .iter()
            .map(|s| format!("{}{}@trade", s.base.to_lowercase(), s.quote.to_lowercase()))
            .collect::<Vec<String>>()
            .join("/");

        let candle_params = tickers
            .iter()
            .map(|s| {
                format!(
                    "{}{}@kline_1m",
                    s.base.to_lowercase(),
                    s.quote.to_lowercase()
                )
            })
            .collect::<Vec<String>>()
            .join("/");

        let depth_params = tickers
            .iter()
            .map(|s| {
                format!(
                    "{}{}@depth{}",
                    s.base.to_lowercase(),
                    s.quote.to_lowercase(),
                    5
                )
            })
            .collect::<Vec<String>>()
            .join("/");

        let request = format!(
            "{}/stream?streams={}/{}",
            *STREAM_ENDPOINT, candle_params, depth_params
        );

        loop {
            let mut ws_stream;
            let response;

            loop {
                info!("Connecting to market data stream {request}");
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

            info!("Connected to market data stream {request}");
            for (header, _value) in response.headers() {
                debug!("\t{header}");
            }

            loop {
                match tokio::time::timeout(Duration::from_secs(60), ws_stream.next()).await {
                    Ok(Some(message)) => match message {
                        Ok(Message::Text(message)) => {
                            debug!("{:?}", message);
                            match serde_json::de::from_slice::<Value>(message.as_ref()) {
                                Ok(value) => match (value.get("data"), value.get("stream")) {
                                    (Some(value), Some(Value::String(stream))) => {
                                        match value.get("e") {
                                            Some(Value::String(e)) if *e == "trade".to_string() => {
                                                match serde_json::from_value::<TradeStream>(
                                                    value.clone(),
                                                ) {
                                                    Ok(trade) => {
                                                        match Ticker::try_from(&trade.symbol) {
                                                            Ok(ticker) => {
                                                                let _ =
                                                                    tx.send(AppEvent::MarketPlace(
                                                                        MarketplaceEvent::Trade(
                                                                            MarketplaceTrade {
                                                                                ticker,
                                                                                price: trade.price,
                                                                                quantity: trade
                                                                                    .quantity,
                                                                                trade_id: trade
                                                                                    .trade_id,
                                                                                trade_time: trade
                                                                                    .trade_time,
                                                                            },
                                                                        ),
                                                                    ));
                                                            }
                                                            Err(..) => {
                                                                error!("Stream parsing error : failed to parse ticker {}", trade.symbol);
                                                            }
                                                        }
                                                    }
                                                    Err(err) => {
                                                        error!("Stream parsing error : {}", err);
                                                    }
                                                }
                                            }
                                            Some(Value::String(e)) if *e == "kline".to_string() => {
                                                match serde_json::from_value::<KLineStream>(
                                                    value.clone(),
                                                ) {
                                                    Ok(candle) => {
                                                        match Ticker::try_from(&candle.symbol) {
                                                            Ok(ticker) => {
                                                                let _ =
                                                                    tx.send(AppEvent::MarketPlace(
                                                                        MarketplaceEvent::Candle(
                                                                            MarketplaceCandle {
                                                                                ticker,
                                                                                high_price: candle
                                                                                    .data
                                                                                    .high_price,
                                                                                low_price: candle
                                                                                    .data
                                                                                    .low_price,
                                                                                start_time: candle
                                                                                    .data
                                                                                    .start_time,
                                                                                close_time: candle
                                                                                    .data
                                                                                    .close_time,
                                                                                trade_count: candle
                                                                                    .data
                                                                                    .trade_count,
                                                                                volume: candle
                                                                                    .data
                                                                                    .volume,
                                                                                closed: candle
                                                                                    .data
                                                                                    .closed,
                                                                                open_price: candle
                                                                                    .data
                                                                                    .open_price,
                                                                                close_price: candle
                                                                                    .data
                                                                                    .close_price,
                                                                            },
                                                                        ),
                                                                    ));
                                                            }
                                                            Err(..) => {
                                                                error!("Stream parsing error : failed to parse ticker {}", candle.symbol);
                                                            }
                                                        }
                                                    }
                                                    Err(err) => {
                                                        error!("Stream parsing error : {}", err);
                                                    }
                                                }
                                            }
                                            Some(Value::String(e))
                                                if *e == "depthUpdate".to_string() =>
                                            {
                                                match serde_json::from_value::<DepthUpdateStream>(
                                                    value.clone(),
                                                ) {
                                                    Ok(depth) => {
                                                        match Ticker::try_from(&depth.symbol) {
                                                            Ok(ticker) => {
                                                                let _ = tx.send(AppEvent::MarketPlace(
                                                                    MarketplaceEvent::Book(MarketplaceBook {
                                                                        ticker,
                                                                        first_update_id: depth.first_update_id,
                                                                        final_update_id: depth.final_update_id,
                                                                        asks: depth.asks,
                                                                        bids: depth.bids,
                                                                        time: Utc::now().timestamp_millis()
                                                                        as u64,
                                                                    }),
                                                                ));
                                                            }
                                                            Err(..) => {
                                                                error!("Stream parsing error : failed to parse ticker {}", depth.symbol);
                                                            }
                                                        }
                                                    }
                                                    Err(err) => {
                                                        error!("Stream parsing error : {}", err);
                                                    }
                                                }
                                            }
                                            _ => {
                                                if stream.contains("depth") {
                                                    if let Some(ticker) =
                                                        extract_ticker_from_stream(stream)
                                                    {
                                                        match serde_json::from_value::<
                                                            PartialDepthStream,
                                                        >(
                                                            value.clone()
                                                        ) {
                                                            Ok(depth) => {
                                                                let _ = tx.send(AppEvent::MarketPlace(
                                                                    MarketplaceEvent::Book(MarketplaceBook {
                                                                        ticker: ticker.clone(),
                                                                        first_update_id: 0,
                                                                        final_update_id: depth.last_update_id,
                                                                        asks: depth.asks,
                                                                        bids: depth.bids,
                                                                        time: Utc::now().timestamp_millis()
                                                                        as u64,
                                                                    }),
                                                                ));
                                                            }
                                                            Err(err) => {
                                                                error!(
                                                                    "Stream parsing error : {}",
                                                                    err
                                                                );
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    _ => {
                                        error!("Unknown json");
                                    }
                                },
                                Err(err) => {
                                    error!("Invalid json");
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
                        error!("Stream timeout: {}", err);
                        break;
                    }
                }
            }

            info!("Reconnecting");
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    }
}

fn extract_ticker_from_stream(stream: &str) -> Option<Ticker> {
    let parts: Vec<&str> = stream.split('@').collect();
    if !parts.is_empty() {
        Ticker::try_from(parts[0]).ok()
    } else {
        None
    }
}
