use crate::marketplace::MarketplaceEvent;
use crate::{marketplace::MarketplaceDataStream, ticker::Ticker, AppEvent};
use anyhow::Result;
use std::time::Duration;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, BufReader};
use tracing::debug;

use super::ReplayMarketplace;

impl<F> MarketplaceDataStream for ReplayMarketplace<F> {
    async fn start_data_stream(
        &mut self,
        tickers: &Vec<Ticker>,
        tx: tokio::sync::broadcast::Sender<AppEvent>,
    ) -> Result<()> {
        let mut path = self.data_path.clone();
        path.push("events.jsonl");

        let file = File::open(path).await?;
        let reader = BufReader::new(file);
        let mut lines = reader.lines();

        while let Ok(Some(line)) = lines.next_line().await {
            while *self.paused.read().await {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            match serde_json::de::from_str::<MarketplaceEvent>(&line) {
                Ok(event) => {
                    if let Some(event_ticker) = event.get_ticker() {
                        if !tickers.contains(event_ticker) {
                            continue;
                        }
                    }
                    if matches!(
                        event,
                        MarketplaceEvent::Book(..)
                            | MarketplaceEvent::Trade(..)
                            | MarketplaceEvent::Candle(..)
                    ) {
                        tx.send(AppEvent::MarketPlace(event)).unwrap();
                    }

                    if self.read_interval > 0 {
                        tokio::time::sleep(Duration::from_micros(self.read_interval)).await;
                    }
                }
                Err(err) => {
                    debug!("Could not parse line : {err}");
                }
            }
        }

        Ok(())
    }
}
