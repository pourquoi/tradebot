use anyhow::Result;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::Sender;

use crate::{order::Order, ticker::Ticker};

pub mod binance;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum MarketPlaceEvent {
    #[serde(rename = "P")]
    Trade(TradeEvent),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TradeEvent {
    #[serde(rename = "t")]
    pub trade_id: u64,
    #[serde(rename = "T")]
    pub trade_time: u64,
    #[serde(rename = "s")]
    pub ticker: Ticker,
    #[serde(with = "rust_decimal::serde::str")]
    #[serde(rename = "p")]
    pub price: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    #[serde(rename = "q")]
    pub quantity: Decimal,
}

pub trait MarketPlace {
    async fn init(&mut self, tickers: Vec<Ticker>) -> Result<()>;
    async fn start(&mut self, tickers: Vec<Ticker>, tx: Sender<MarketPlaceEvent>);
    async fn ping(&self) -> Result<()>;
    fn get_fees(order: &Order) -> Decimal;
    fn adjust_order_price_and_amount(&self, order: &mut Order) -> Result<()>;
}
