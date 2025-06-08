use crate::{
    marketplace::binance::{Binance, ENDPOINT},
    ticker::Ticker,
};
use anyhow::Result;
use reqwest::Url;
use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer};
use tracing::{debug, info};

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ExchangeInfo {
    pub symbols: Vec<SymbolInfo>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SymbolInfo {
    pub symbol: String,
    pub base_asset: String,
    pub quote_asset: String,
    pub base_asset_precision: u8,
    pub quote_asset_precision: u8,
    #[serde(deserialize_with = "deserialize_filters")]
    pub filters: Vec<SymbolInfoFilter>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
#[serde(tag = "filterType")]
pub enum SymbolInfoFilter {
    #[serde(rename(deserialize = "PRICE_FILTER"))]
    #[serde(rename_all = "camelCase")]
    PriceFilter {
        #[serde(with = "rust_decimal::serde::str")]
        min_price: Decimal,
        #[serde(with = "rust_decimal::serde::str")]
        max_price: Decimal,
        #[serde(with = "rust_decimal::serde::str")]
        tick_size: Decimal,
    },
    #[serde(rename = "LOT_SIZE")]
    #[serde(rename_all = "camelCase")]
    LotSize {
        #[serde(with = "rust_decimal::serde::str")]
        min_qty: Decimal,
        #[serde(with = "rust_decimal::serde::str")]
        max_qty: Decimal,
        #[serde(with = "rust_decimal::serde::str")]
        step_size: Decimal,
    },
    #[serde(rename = "NOTIONAL")]
    #[serde(rename_all = "camelCase")]
    Notional {
        #[serde(with = "rust_decimal::serde::str")]
        min_notional: Decimal,
    },
}

fn deserialize_filters<'de, D>(deserializer: D) -> Result<Vec<SymbolInfoFilter>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw: Vec<serde_json::Value> = Vec::deserialize(deserializer)?;
    let mut filters = Vec::new();

    for item in raw {
        match serde_json::from_value::<SymbolInfoFilter>(item.clone()) {
            Ok(filter) => {
                filters.push(filter);
            }
            Err(err) => {
                debug!("Failed deserialize symbol info {:?} {}", item, err);
            }
        }
    }

    Ok(filters)
}

impl Binance {
    pub async fn get_exchange_info(&self, tickers: &[Ticker]) -> Result<ExchangeInfo> {
        let symbols_param: Vec<String> = tickers
            .iter()
            .map(|ticker| format!("\"{}{}\"", ticker.base, ticker.quote))
            .collect();
        let symbols_param: String = symbols_param.join(",");
        let symbols_param = format!("[{}]", symbols_param);
        let url_params = [("symbols", symbols_param)];

        let url = Url::parse_with_params(
            format!("{}/api/v3/exchangeInfo", ENDPOINT).as_str(),
            url_params,
        )
        .unwrap();

        info!("{}", url);

        let r = self.client.get(url).send().await?;
        let r = r.text().await.unwrap();
        let r: ExchangeInfo = serde_json::de::from_str(r.as_str()).unwrap();

        Ok(r)
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
