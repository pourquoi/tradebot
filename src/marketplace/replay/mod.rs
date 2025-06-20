use crate::ticker::Ticker;
use std::{path::PathBuf, sync::Arc};
use tokio::fs::OpenOptions;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncWriteExt},
    sync::RwLock,
};

use super::{
    Marketplace, MarketplaceCandle, MarketplaceDataApi, MarketplaceEvent, MarketplaceSettingsApi,
};

pub mod data_stream;

#[derive(Clone, Debug)]
pub struct ReplayMarketplace<F> {
    data_path: PathBuf,
    paused: Arc<RwLock<bool>>,
    read_interval: u64,
    fallback: F,
}

impl<F: MarketplaceSettingsApi + MarketplaceDataApi> ReplayMarketplace<F> {
    pub fn new(data_path: PathBuf, fallback: F) -> Self {
        Self {
            data_path,
            paused: Arc::new(RwLock::new(false)),
            read_interval: 1,
            fallback,
        }
    }

    pub async fn get_start_time(&self) -> Option<u64> {
        let mut path = self.data_path.clone();
        path.push("events.jsonl");

        let file = File::open(path).await.ok()?;
        let reader = BufReader::new(file);
        let mut lines = reader.lines();

        while let Ok(Some(line)) = lines.next_line().await {
            if let Ok(event) = serde_json::de::from_str::<MarketplaceEvent>(&line) {
                return event.get_time();
            }
        }
        None
    }

    pub async fn toggle_pause(&mut self) {
        let mut paused = self.paused.write().await;
        *paused = !*paused;
    }
}

impl<F> Marketplace for ReplayMarketplace<F> {}

impl<F: MarketplaceSettingsApi> MarketplaceSettingsApi for ReplayMarketplace<F> {
    async fn get_fees(&self) -> rust_decimal::Decimal {
        self.fallback.get_fees().await
    }

    async fn adjust_order_price_and_amount(
        &self,
        order: &mut crate::order::Order,
    ) -> anyhow::Result<()> {
        self.fallback.adjust_order_price_and_amount(order).await
    }
}

impl<F: MarketplaceDataApi> MarketplaceDataApi for ReplayMarketplace<F> {
    async fn get_candles(
        &self,
        ticker: &Ticker,
        interval: &str,
        from: Option<u64>,
        to: Option<u64>,
    ) -> anyhow::Result<Vec<MarketplaceCandle>> {
        let cache_path = if from.is_some() || to.is_some() {
            let mut path = self.data_path.clone();
            path.push(format!(
                "candles-{}-{}-{}-{}.json",
                ticker,
                interval,
                from.unwrap_or(0),
                to.unwrap_or(0)
            ));
            Some(path)
        } else {
            None
        };

        let cache = if let Some(cache_path) = &cache_path {
            File::open(cache_path).await.ok()
        } else {
            None
        };

        if let Some(mut cache) = cache {
            let mut content = String::new();
            cache.read_to_string(&mut content).await?;
            let candles = serde_json::from_str::<Vec<MarketplaceCandle>>(content.as_str())?;
            return Ok(candles);
        } else {
            let candles = self
                .fallback
                .get_candles(ticker, interval, from, to)
                .await?;
            if let Some(cache_path) = &cache_path {
                let mut cache = OpenOptions::new()
                    .write(true)
                    .create(true)
                    .open(cache_path)
                    .await?;
                let content = serde_json::to_string(&candles)?;
                cache.write(content.as_bytes()).await?;
            }
            return Ok(candles);
        }
    }
}
