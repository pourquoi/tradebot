use anyhow::{anyhow, Result};
use reqwest::Url;
use rust_decimal::prelude::*;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::ser::SerializeSeq;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use tracing::info;

use crate::marketplace::binance::{Binance, ENDPOINT, PUBLIC_ENDPOINT};
use crate::marketplace::MarketplaceCandle;
use crate::ticker::Ticker;

#[derive(Deserialize, Debug, Clone, Default)]
#[allow(dead_code)]
pub struct Candle {
    pub start_time: u64,
    pub close_time: u64,
    pub open_price: Decimal,
    pub close_price: Decimal,
    pub high_price: Decimal,
    pub low_price: Decimal,
    pub trade_count: u64,
    pub volume: Decimal,
}

impl Candle {
    pub fn into_candle_event(self, ticker: Ticker) -> MarketplaceCandle {
        MarketplaceCandle {
            ticker,
            open_price: self.open_price,
            close_price: self.close_price,
            high_price: self.high_price,
            low_price: self.low_price,
            trade_count: self.trade_count,
            start_time: self.start_time,
            close_time: self.close_time,
            volume: self.volume,
            closed: true,
        }
    }
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
            start_time: value[0]
                .as_u64()
                .ok_or_else(|| anyhow!("Invalid open time"))?,
            close_time: value[6].as_u64().ok_or(anyhow!("Invalid close time"))?,
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

impl Serialize for Candle {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut seq = serializer.serialize_seq(Some(12))?;
        // Kline open time
        seq.serialize_element(&self.start_time)?;
        // Open price
        seq.serialize_element(&self.open_price)?;
        // High price
        seq.serialize_element(&self.high_price)?;
        // Low price
        seq.serialize_element(&self.low_price)?;
        // Close price
        seq.serialize_element(&self.close_price)?;
        // Volume
        seq.serialize_element(&self.volume)?;
        // Kline close time
        seq.serialize_element(&self.close_time)?;
        // Quote asset volume
        seq.serialize_element(&self.volume)?;
        // Number of trades
        seq.serialize_element(&self.trade_count)?;
        // Taker buy base asset volume
        seq.serialize_element("")?;
        // Taker buy quote asset volume
        seq.serialize_element("")?;
        // Unused field. Ignore.
        seq.serialize_element("")?;

        seq.end()
    }
}

#[derive(Deserialize, Debug, Clone)]
#[allow(dead_code)]
pub struct Depth {
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

impl Binance {
    pub async fn get_candles(
        &self,
        symbol: &str,
        interval: &str,
        from: Option<u64>,
        to: Option<u64>,
    ) -> Result<Vec<Candle>> {
        let mut params = vec![
            ("symbol", symbol.to_string()),
            ("interval", interval.to_string()),
        ];
        if let Some(from) = from {
            params.push(("startTime", from.to_string()));
        }
        if let Some(to) = to {
            params.push(("endTime", to.to_string()));
        }
        let url = Url::parse_with_params(
            format!("{}/api/v3/klines", *PUBLIC_ENDPOINT).as_str(),
            &params,
        )
        .unwrap();

        info!("{}", url);

        let r = self.client.get(url).send().await?;
        let r: Vec<Value> = r.json().await?;
        Ok(r.into_iter()
            .flat_map(|v| v.try_into())
            .collect::<Vec<Candle>>())
    }

    pub async fn get_depth(&self, ticker: &Ticker) -> Result<Depth> {
        let url = format!("{}/api/v3/depth?symbol={}", *ENDPOINT, ticker);
        info!("{}", url);
        let r = self.client.get(url).send().await?;
        let depth: Depth = r.json().await?;
        Ok(depth)
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

    #[test]
    fn test_candle_to_json() {
        let candle = Candle {
            start_time: 1499040000000_u64,
            open_price: dec!(0.01634790),
            high_price: dec!(0.80000000),
            low_price: dec!(0.01575800),
            close_price: dec!(0.01577100),
            volume: dec!(148976.11427815),
            close_time: 1499644799999_u64,
            trade_count: 308,
        };

        let res = serde_json::ser::to_string(&candle);
        assert!(res.is_ok());
        assert_eq!(res.unwrap(), String::from("[1499040000000,\"0.01634790\",\"0.80000000\",\"0.01575800\",\"0.01577100\",\"148976.11427815\",1499644799999,\"148976.11427815\",308,\"\",\"\",\"\"]"));
    }
}
