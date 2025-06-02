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
    #[serde(rename = "C")]
    Candle(CandleEvent),
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

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CandleEvent {
    #[serde(rename = "s")]
    pub ticker: Ticker,

    #[serde(rename = "o")]
    #[serde(with = "rust_decimal::serde::str")]
    pub open_price: Decimal,

    #[serde(rename = "c")]
    #[serde(with = "rust_decimal::serde::str_option")]
    pub close_price: Option<Decimal>,

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

pub trait MarketPlace {
    async fn init(&mut self, tickers: Vec<Ticker>) -> Result<()>;
    async fn start(&mut self, tickers: Vec<Ticker>, tx: Sender<MarketPlaceEvent>);
    async fn ping(&self) -> Result<()>;
    async fn get_fees(&self, order: &Order) -> Decimal;
    async fn adjust_order_price_and_amount(&self, order: &mut Order) -> Result<()>;
}
