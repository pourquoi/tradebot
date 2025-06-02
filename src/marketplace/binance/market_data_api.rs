use anyhow::{anyhow, Result};
use reqwest::Url;
use rust_decimal::prelude::*;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::ser::SerializeSeq;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::marketplace::binance::{Binance, PUBLIC_MARKET_ENDPOINT};

#[derive(Deserialize, Debug, Clone, Default)]
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

impl Serialize for Candle {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut seq = serializer.serialize_seq(Some(12))?;
        // Kline open time
        seq.serialize_element(&self.open_time)?;
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

impl Binance {
    #[allow(dead_code)]
    pub async fn get_candles(&self, symbol: &str, interval: &str) -> Result<Vec<Candle>> {
        let params = [("symbol", symbol), ("interval", interval)];
        let url = Url::parse_with_params(
            format!("{PUBLIC_MARKET_ENDPOINT}/api/v3/klines").as_str(),
            &params,
        )
        .unwrap();
        println!("Getting candles {}", url);
        let r = self.client.get(url).send().await?;
        let r = r.text().await.unwrap();
        let r: Vec<Value> = serde_json::de::from_str(r.as_str()).unwrap();
        Ok(r.into_iter()
            .flat_map(|v| v.try_into())
            .collect::<Vec<Candle>>())
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
            open_time: 1499040000000_i64,
            open_price: dec!(0.01634790),
            high_price: dec!(0.80000000),
            low_price: dec!(0.01575800),
            close_price: dec!(0.01577100),
            volume: dec!(148976.11427815),
            close_time: 1499644799999_i64,
            trade_count: 308,
        };

        let res = serde_json::ser::to_string(&candle);
        assert!(res.is_ok());
        assert_eq!(res.unwrap(), String::from("[1499040000000,\"0.01634790\",\"0.80000000\",\"0.01575800\",\"0.01577100\",\"148976.11427815\",1499644799999,\"148976.11427815\",308,\"\",\"\",\"\"]"));
    }
}
