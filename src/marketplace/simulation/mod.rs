use crate::marketplace::{Marketplace, MarketplaceBook, MarketplaceSettingsApi};
use crate::order::Order;
use crate::portfolio::Asset;
use crate::ticker::Ticker;
use anyhow::anyhow;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast::Sender;
use tokio::sync::RwLock;
use tracing::error;

use super::{MarketplaceEvent, MarketplaceOrderUpdate, MarketplacePortfolioUpdate};

pub mod account_api;
pub mod account_stream;
pub mod matching;
pub mod trade_api;

#[derive(Debug, Clone)]
pub enum SimulationSource {
    Candles,
    Book,
    Trades,
}

#[derive(Debug, Clone)]
pub struct SimulationMarketplace<S: MarketplaceSettingsApi> {
    source: SimulationSource,
    assets: Arc<RwLock<HashMap<String, Asset>>>,
    orders: Arc<RwLock<Vec<Order>>>,
    order_book: Arc<RwLock<HashMap<Ticker, MarketplaceBook>>>,
    current_time: Arc<RwLock<u64>>,
    settings: S,
    tx_account: Sender<MarketplaceEvent>,
}

impl<S: MarketplaceSettingsApi> SimulationMarketplace<S> {
    pub fn new(source: SimulationSource, settings: S) -> Self {
        let (tx_account, _) = tokio::sync::broadcast::channel(100);
        Self {
            tx_account,
            source,
            assets: Arc::new(Default::default()),
            orders: Arc::new(Default::default()),
            order_book: Arc::new(Default::default()),
            current_time: Arc::new(Default::default()),
            settings,
        }
    }
}

impl<S: MarketplaceSettingsApi> Marketplace for SimulationMarketplace<S> {}

impl<S: MarketplaceSettingsApi> SimulationMarketplace<S> {
    async fn notify_portfolio_update(&self, assets: Vec<Asset>) {
        let time = *self.current_time.read().await;
        if let Err(err) = self
            .tx_account
            .send(super::MarketplaceEvent::PortfolioUpdate(
                MarketplacePortfolioUpdate { time, assets },
            ))
        {
            error!("Failed broadcast portfolio update : {err}");
        };
    }

    async fn notify_order_update(&self, update: MarketplaceOrderUpdate) {
        if let Err(err) = self
            .tx_account
            .send(super::MarketplaceEvent::OrderUpdate(update))
        {
            error!("Failed broadcast order update : {err}");
        };
    }

    async fn lock_funds(&mut self, asset: &str, amount: Decimal) -> anyhow::Result<()> {
        let mut assets = self.assets.write().await;
        match assets.get_mut(asset) {
            Some(asset) => {
                if asset.amount < amount {
                    return Err(anyhow!(
                        "Missing {} for {}",
                        amount - asset.amount,
                        asset.symbol
                    ));
                }
                asset.amount -= amount;
                asset.locked += amount;
                self.notify_portfolio_update(vec![asset.clone()]).await;
                Ok(())
            }
            None => Err(anyhow!("Asset {} not in portfolio", asset)),
        }
    }

    pub async fn update_asset_amount(
        &self,
        symbol: &str,
        delta: Decimal,
        current_price: Option<Decimal>,
    ) {
        let mut assets = self.assets.write().await;
        let asset = assets
            .entry(symbol.to_owned())
            .and_modify(|asset| {
                asset.amount += delta;
                asset.value = current_price.map(|p| (asset.amount + asset.locked) * p);
            })
            .or_insert(Asset {
                symbol: symbol.to_owned(),
                amount: delta,
                locked: dec!(0),
                value: current_price.map(|p| delta * p),
            });
        self.notify_portfolio_update(vec![asset.clone()]).await;
    }

    pub async fn update_asset_locked(
        &self,
        symbol: &str,
        delta: Decimal,
        current_price: Option<Decimal>,
    ) {
        let mut assets = self.assets.write().await;
        let asset = assets
            .entry(symbol.to_owned())
            .and_modify(|asset| {
                asset.locked += delta;
                asset.value = current_price.map(|p| (asset.amount + asset.locked) * p);
            })
            .or_insert(Asset {
                symbol: symbol.to_owned(),
                amount: dec!(0),
                locked: delta,
                value: current_price.map(|p| delta * p),
            });
        self.notify_portfolio_update(vec![asset.clone()]).await;
    }
}
