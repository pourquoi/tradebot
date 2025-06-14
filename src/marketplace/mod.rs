use std::collections::HashMap;

use anyhow::Result;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::Sender;

use crate::{order::Order, portfolio::Asset, ticker::Ticker, AppEvent};

pub mod binance;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum MarketPlaceEvent {
    #[serde(rename = "P")]
    Trade(TradeEvent),
    #[serde(rename = "C")]
    Candle(CandleEvent),
    #[serde(rename = "D")]
    Depth(DepthEvent),
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

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DepthEvent {
    #[serde(rename = "s")]
    pub ticker: Ticker,

    #[serde(rename = "U")]
    pub first_update_id: u64,

    #[serde(rename = "u")]
    pub final_update_id: u64,

    #[serde(rename = "T")]
    pub time: u64,

    #[serde(
        rename = "a",
        deserialize_with = "crate::utils::deserialize_decimal_pairs",
        serialize_with = "crate::utils::serialize_decimal_pairs"
    )]
    pub asks: Vec<(Decimal, Decimal)>,

    #[serde(
        rename = "b",
        deserialize_with = "crate::utils::deserialize_decimal_pairs",
        serialize_with = "crate::utils::serialize_decimal_pairs"
    )]
    pub bids: Vec<(Decimal, Decimal)>,
}

impl DepthEvent {
    pub fn buy_price(&self) -> Option<Decimal> {
        self.bids.first().map(|(price, _)| *price)
    }
    pub fn sell_price(&self) -> Option<Decimal> {
        self.asks.first().map(|(price, _)| *price)
    }
}

pub trait MarketPlace {}

pub trait MarketPlaceStream {
    fn start(
        &mut self,
        tickers: &Vec<Ticker>,
        tx: Sender<AppEvent>,
    ) -> impl std::future::Future<Output = ()>;
}

pub trait MarketPlaceData {
    fn get_candles(
        &self,
        ticker: &Ticker,
        interval: &str,
        from: Option<u64>,
        to: Option<u64>,
    ) -> impl std::future::Future<Output = Result<Vec<CandleEvent>>>;
}

pub trait MarketPlaceSettings {
    // Get the fees ratio for this order.
    fn get_fees(&self) -> impl std::future::Future<Output = Decimal>;

    // Adjust the price and amount rounding according the marketplace settings.
    fn adjust_order_price_and_amount(
        &self,
        order: &mut Order,
    ) -> impl std::future::Future<Output = Result<()>>;
}

pub trait MarketPlaceAccount {
    fn get_account_assets(
        &mut self,
    ) -> impl std::future::Future<Output = Result<HashMap<String, Asset>>>;
}

pub trait MarketPlaceTrade {
    fn get_orders(
        &self,
        tickers: &Vec<Ticker>,
    ) -> impl std::future::Future<Output = Result<Vec<Order>>>;
    fn place_order(&self, order: &Order) -> impl std::future::Future<Output = Result<Order>>;
}
