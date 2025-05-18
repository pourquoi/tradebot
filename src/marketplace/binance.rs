use std::time::Duration;

use anyhow::{anyhow, Result};
use futures::SinkExt;
use futures_util::StreamExt;
use reqwest::{Client, Response, Url};
use rust_decimal::prelude::*;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::broadcast::Sender;
use tokio_tungstenite::connect_async;
use tracing::debug;
use tracing::error;
use tracing::info;
use tungstenite::Message;

const ENDPOINT: &str = "https://api.binance.com";

#[derive(Deserialize, Clone, Debug)]
#[allow(dead_code)]
pub struct MultiStream<T> {
    pub stream: String,
    pub data: T,
}

#[derive(Deserialize, Clone, Debug)]
#[allow(dead_code)]
pub struct MarkPriceStream {
    #[serde(rename(deserialize = "e"))]
    pub event_type: String,

    #[serde(rename(deserialize = "E"))]
    pub event_time: u64,

    #[serde(rename(deserialize = "s"))]
    pub symbol: String,

    #[serde(rename(deserialize = "p"))]
    #[serde(with = "rust_decimal::serde::str")]
    pub mark_price: Decimal,

    #[serde(rename(deserialize = "i"))]
    #[serde(with = "rust_decimal::serde::str")]
    pub index_price: Decimal,
}

#[derive(Deserialize, Clone, Debug)]
#[allow(dead_code)]
pub struct KLineStream {
    #[serde(rename(deserialize = "e"))]
    pub event_type: String,

    #[serde(rename(deserialize = "E"))]
    pub event_time: u64,

    #[serde(rename(deserialize = "s"))]
    pub symbol: String,

    #[serde(rename(deserialize = "k"))]
    pub data: KLineData,
}

#[derive(Deserialize, Clone, Debug)]
#[allow(dead_code)]
pub struct KLineData {
    #[serde(rename(deserialize = "h"))]
    #[serde(with = "rust_decimal::serde::str")]
    pub high_price: Decimal,

    #[serde(rename(deserialize = "l"))]
    #[serde(with = "rust_decimal::serde::str")]
    pub low_price: Decimal,

    #[serde(rename(deserialize = "n"))]
    pub trade_count: u64,

    #[serde(rename(deserialize = "t"))]
    pub start_time: u64,

    #[serde(rename(deserialize = "T"))]
    pub close_time: u64,
}

#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct Candle {
    pub open_time: i64,
    pub close_time: i64,
    pub open_price: Decimal,
    pub close_price: Decimal,
    pub high_price: Decimal,
    pub low_price: Decimal,
    pub trade_count: u32,
    pub volume: Decimal,
}

impl TryFrom<Value> for Candle {
    type Error = anyhow::Error;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        let value = if let Value::Array(value) = value {
            value
        } else {
            return Err(anyhow!("Expected an array for candle data"));
        };
        if value.len() < 11 {
            return Err(anyhow!("Candle array needs at least 11 elements"));
        }
        let candle = Candle {
            open_time: value[0]
                .as_i64()
                .ok_or_else(|| anyhow!("Invalid open time"))?,
            close_time: value[6].as_i64().ok_or(anyhow!("Invalid close time"))?,
            open_price: value[1]
                .as_str()
                .ok_or(anyhow!("Invalid open price"))
                .and_then(|v| {
                    Decimal::from_str(v).map_err(|e| anyhow!("Invalid decimal: {}", e))
                })?,
            close_price: value[4]
                .as_str()
                .ok_or(anyhow!("Invalid close price"))
                .and_then(|v| {
                    Decimal::from_str(v).map_err(|e| anyhow!("Invalid decimal: {}", e))
                })?,
            high_price: value[2]
                .as_str()
                .ok_or(anyhow!("Invalid high price"))
                .and_then(|v| {
                    Decimal::from_str(v).map_err(|e| anyhow!("Invalid decimal: {}", e))
                })?,
            low_price: value[3]
                .as_str()
                .ok_or(anyhow!("Invalid low price"))
                .and_then(|v| {
                    Decimal::from_str(v).map_err(|e| anyhow!("Invalid decimal: {}", e))
                })?,
            trade_count: 0,
            volume: dec!(0),
        };
        Ok(candle)
    }
}

pub struct Binance {
    client: Client,
}

impl Binance {
    pub fn new() -> Self {
        let client = Client::builder().build().unwrap();
        Self { client }
    }

    #[allow(dead_code)]
    pub async fn candles(&self, symbol: &str, interval: &str) -> Result<Vec<Candle>> {
        let params = [("symbol", symbol), ("interval", interval)];
        let url =
            Url::parse_with_params(format!("{ENDPOINT}/api/v3/klines").as_str(), &params).unwrap();
        println!("{}", url);
        let r = self.client.get(url).send().await?;
        let r = r.text().await.unwrap();
        let r: Vec<Value> = serde_json::de::from_str(r.as_str()).unwrap();
        Ok(r.into_iter()
            .flat_map(|v| v.try_into())
            .collect::<Vec<Candle>>())
    }
}

impl crate::marketplace::MarketPlace for Binance {
    async fn start(&self, tickers: Vec<String>, tx: Sender<crate::marketplace::MarketPlaceEvent>) {
        let params = tickers
            .iter()
            //.map(|s| format!("{}@kline_1m", s))
            .map(|s| format!("{}@markPrice@1s", s))
            .collect::<Vec<String>>()
            .join("/");
        let request = format!("wss://fstream.binance.com/stream?streams={}", params);
        loop {
            tokio::time::sleep(Duration::from_secs(3)).await;
            let (mut ws_stream, response) = connect_async(request.clone())
                .await
                .expect("Failed to connect to websocket");
            info!("Connected to websocket");
            for (header, _value) in response.headers() {
                debug!("* {header}");
            }

            loop {
                while let Some(message) = ws_stream.next().await {
                    match message {
                        Ok(Message::Text(message)) => {
                            match serde_json::de::from_slice::<MultiStream<MarkPriceStream>>(
                                message.as_ref(),
                            ) {
                                Ok(stream) => {
                                    let _ =
                                        tx.send(crate::marketplace::MarketPlaceEvent::TickerPrice(
                                            crate::marketplace::TickerPriceEvent {
                                                ticker: stream.data.symbol,
                                                price: stream.data.mark_price,
                                            },
                                        ));
                                }
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
                    }
                }
            }
        }
    }

    async fn ping(&self) -> Result<()> {
        let _ = self
            .client
            .get(format!("{ENDPOINT}/api/v3/ping"))
            .send()
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn test_candle_from_json() {
        let json = json!([
            1499040000000_u64, // Kline open time
            "0.01634790",      // Open price
            "0.80000000",      // High price
            "0.01575800",      // Low price
            "0.01577100",      // Close price
            "148976.11427815", // Volume
            1499644799999_u64, // Kline close time
            "2434.19055334",   // Quote asset volume
            308,               // Number of trades
            "1756.87402397",   // Taker buy base asset volume
            "28.46694368",     // Taker buy quote asset volume
            "0"                // Unused field. Ignore.
        ]);
        let res = Candle::try_from(json);
        assert!(res.is_ok());
        let candle = res.unwrap();
        assert_eq!(candle.low_price, dec!(0.01575800))
    }
}
