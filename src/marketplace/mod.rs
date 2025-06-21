use std::collections::HashMap;

use anyhow::Result;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::Sender;

use crate::{
    order::{Order, OrderStatus, OrderTrade},
    portfolio::Asset,
    ticker::Ticker,
    AppEvent,
};

pub mod binance;
pub mod replay;
pub mod simulation;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum MarketplaceEvent {
    #[serde(rename = "P")]
    Trade(MarketplaceTrade),
    #[serde(rename = "C")]
    Candle(MarketplaceCandle),
    #[serde(rename = "D")]
    Book(MarketplaceBook),
    #[serde(rename = "A")]
    PortfolioUpdate(MarketplacePortfolioUpdate),
    #[serde(rename = "O")]
    OrderUpdate(MarketplaceOrderUpdate),
}

impl MarketplaceEvent {
    fn get_ticker(&self) -> Option<&Ticker> {
        match &self {
            Self::Trade(event) => Some(&event.ticker),
            Self::Candle(event) => Some(&event.ticker),
            Self::Book(event) => Some(&event.ticker),
            _ => None,
        }
    }

    fn get_time(&self) -> Option<u64> {
        match &self {
            Self::Trade(event) => Some(event.trade_time),
            Self::Candle(event) => Some(event.close_time),
            Self::Book(event) => Some(event.time),
            _ => None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MarketplaceTrade {
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
pub struct MarketplaceCandle {
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
pub struct MarketplaceBook {
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

impl MarketplaceBook {
    pub fn buy_price(&self) -> Option<Decimal> {
        self.bids.first().map(|(price, _)| *price)
    }
    pub fn sell_price(&self) -> Option<Decimal> {
        self.asks.first().map(|(price, _)| *price)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MarketplacePortfolioUpdate {
    #[serde(rename = "E")]
    pub time: u64,
    #[serde(rename = "B")]
    pub assets: Vec<Asset>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MarketplaceOrderUpdate {
    #[serde(rename = "E")]
    pub time: u64,
    #[serde(rename = "x")]
    pub update_type: String,
    #[serde(rename = "i")]
    pub marketplace_id: String,
    #[serde(rename = "c")]
    pub client_id: String,
    #[serde(rename = "s")]
    pub status: OrderStatus,
    #[serde(rename = "W")]
    pub working_time: Option<u64>,
    #[serde(rename = "T")]
    pub trade: Option<OrderTrade>,
}

pub trait Marketplace {}

pub trait MarketplaceDataStream {
    fn start_data_stream(
        &mut self,
        tickers: &Vec<Ticker>,
        tx: Sender<AppEvent>,
    ) -> impl std::future::Future<Output = anyhow::Result<()>>;
}

pub trait MarketplaceAccountStream {
    fn start_account_stream(
        &mut self,
        tx: Sender<AppEvent>,
    ) -> impl std::future::Future<Output = anyhow::Result<()>>;
}

pub trait MarketplaceDataApi {
    fn get_candles(
        &self,
        ticker: &Ticker,
        interval: &str,
        from: Option<u64>,
        to: Option<u64>,
    ) -> impl std::future::Future<Output = Result<Vec<MarketplaceCandle>>>;
}

pub trait MarketplaceSettingsApi {
    // Get the fees ratio for this order.
    fn get_fees(&self) -> impl std::future::Future<Output = Decimal>;

    // Adjust the price and amount rounding according the marketplace settings.
    fn adjust_order_price_and_amount(
        &self,
        order: &mut Order,
    ) -> impl std::future::Future<Output = Result<()>>;
}

pub trait MarketplaceAccountApi {
    fn get_account_assets(
        &mut self,
    ) -> impl std::future::Future<Output = Result<HashMap<String, Asset>>>;
}

pub trait MarketplaceTradeApi {
    fn get_orders(
        &self,
        tickers: &[Ticker],
    ) -> impl std::future::Future<Output = Result<Vec<Order>>>;

    fn place_order(&mut self, order: &Order) -> impl std::future::Future<Output = Result<Order>>;
}

pub trait MarketplaceMatching {
    fn start_matching(
        &mut self,
        app_tx: Sender<AppEvent>,
    ) -> impl std::future::Future<Output = Result<()>>;
}
