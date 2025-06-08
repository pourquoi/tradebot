use std::env;

use anyhow::Result;
use chrono::prelude::*;
use hex::encode;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::Deserialize;
use sha2::Sha256;
use tracing::{debug, info};

use crate::{
    marketplace::binance::Binance,
    order::{Order, OrderSide, OrderStatus, OrderType},
    ticker::Ticker,
};

use super::ENDPOINT;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderResponse {
    pub symbol: String,
    pub order_id: u64,
    pub order_list_id: i64,
    pub client_order_id: String,
    pub transact_time: u64,
    pub price: Decimal,
    pub orig_qty: Decimal,
    pub executed_qty: Decimal,
    pub cummulative_quote_qty: Decimal,
    pub status: String,
    pub time_in_force: String,

    #[serde(rename = "type")]
    pub order_type: String,

    pub side: String,
    pub working_time: Option<u64>,

    #[serde(default)]
    pub fills: Vec<Fill>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fill {
    pub price: Decimal,
    pub qty: Decimal,
    pub commission: Decimal,

    pub commission_asset: Decimal,

    #[serde(default)]
    pub trade_id: Option<u64>,
}

impl TryFrom<&OrderResponse> for Order {
    type Error = String;

    fn try_from(value: &OrderResponse) -> std::result::Result<Self, Self::Error> {
        let order = Order {
            creation_time: value.transact_time,
            sent_time: Some(value.transact_time),
            working_time: value.working_time,
            ticker: Ticker::try_from(&value.symbol)?,
            status: OrderStatus::try_from(&value.status)?,
            order_type: OrderType::try_from(&value.order_type)?,
            side: OrderSide::try_from(&value.side)?,
            price: value.price,
            amount: value.orig_qty,
            filled_amount: value.executed_qty,
            trades: Vec::new(),
            parent_order_price: None,
            marketplace_id: Some(format!("{}", value.order_id)),
        };

        Ok(order)
    }
}

impl TryFrom<OrderResponse> for Order {
    type Error = String;

    fn try_from(value: OrderResponse) -> std::result::Result<Self, Self::Error> {
        Order::try_from(&value)
    }
}

impl Binance {
    pub async fn get_open_orders(&self, ticker: &Ticker) -> Result<Vec<OrderResponse>> {
        let api_key = env::var("BINANCE_API_KEY")?;
        let api_secret = env::var("BINANCE_API_SECRET")?;

        let timestamp = Utc::now().timestamp_millis();
        let params = format!("timestamp={}&symbol={}", timestamp, ticker);

        let mut mac: Hmac<Sha256> = Hmac::new_from_slice(api_secret.as_bytes())?;
        mac.update(params.as_bytes());
        let signature = encode(mac.finalize().into_bytes());

        let url = format!(
            "{}/api/v3/openOrders?{}&signature={}",
            ENDPOINT, params, signature
        );

        info!("{}", url);

        let res = self
            .client
            .get(&url)
            .header("X-MBX-APIKEY", api_key)
            .send()
            .await?;

        let orders_response: Vec<OrderResponse> = res.json().await?;

        debug!("Binance {} open orders : {:?}", ticker, orders_response);

        Ok(orders_response)
    }

    pub async fn place_order(&self, order: &Order) -> Result<OrderResponse> {
        let api_key = env::var("BINANCE_API_KEY")?;
        let api_secret = env::var("BINANCE_API_SECRET")?;

        let timestamp = Utc::now().timestamp_millis();
        let params = format!(
            "timestamp={}&symbol={}&side={}&order_type={}&quantity={}&price={}&timeInForce={}",
            timestamp, order.ticker, order.order_type, "LIMIT", order.amount, order.price, "GTC"
        );

        let mut mac: Hmac<Sha256> = Hmac::new_from_slice(api_secret.as_bytes())?;
        mac.update(params.as_bytes());
        let signature = encode(mac.finalize().into_bytes());

        let url = format!(
            "{}/api/v3/order?{}&signature={}",
            ENDPOINT, params, signature
        );

        info!("{}", url);

        let res = self
            .client
            .post(&url)
            .header("X-MBX-APIKEY", api_key)
            .send()
            .await?;

        let order_response: OrderResponse = res.json().await?;

        debug!("Binance order response : {:?}", order_response);

        Ok(order_response)
    }
}
