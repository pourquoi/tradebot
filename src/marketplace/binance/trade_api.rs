use std::env;

use crate::{
    marketplace::binance::Binance,
    order::{Order, OrderSide, OrderStatus, OrderTrade, OrderType},
    ticker::Ticker,
};
use anyhow::Result;
use chrono::prelude::*;
use hex::encode;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::Deserialize;
use sha2::Sha256;
use tracing::{debug, error, info};

use super::ENDPOINT;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderResponse {
    pub symbol: String,
    pub order_id: u64,
    pub order_list_id: i64,
    pub client_order_id: String,
    pub transact_time: u64,
    #[serde(with = "rust_decimal::serde::str")]
    pub price: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub orig_qty: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub orig_quote_order_qty: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub executed_qty: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
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
    #[serde(with = "rust_decimal::serde::str")]
    pub price: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub qty: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub commission: Decimal,
    pub commission_asset: String,
    pub trade_id: u64,
}

impl TryFrom<&OrderResponse> for Order {
    type Error = String;

    fn try_from(value: &OrderResponse) -> std::result::Result<Self, Self::Error> {
        let mut order = Order {
            id: value.client_order_id.clone(),
            fees: dec!(0.001),
            profit: dec!(0),
            creation_time: value.transact_time,
            working_time: value.working_time,
            ticker: Ticker::try_from(&value.symbol)?,
            status: OrderStatus::try_from(&value.status)?,
            order_type: OrderType::try_from(&value.order_type)?,
            quote_amount: value.orig_quote_order_qty,
            side: OrderSide::try_from(&value.side)?,
            price: value.price,
            amount: value.orig_qty,
            filled_amount: value.executed_qty,
            cumulative_quote_amount: value.cummulative_quote_qty,
            trades: Vec::new(),
            buy_order_price: None,
            sell_order_price: None,
            marketplace_id: Some(value.order_id.to_string()),
            session_id: None,
            strategy: None,
            next_order_id: None,
            prev_order_id: None,
            reject_reason: None
        };

        order.trades = value
            .fills
            .iter()
            .map(|fill| OrderTrade {
                id: fill.trade_id.to_string(),
                trade_time: value.transact_time,
                price: fill.price,
                amount: fill.qty,
            })
            .collect();

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
            *ENDPOINT, params, signature
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
        let mut params = format!(
            "timestamp={}&symbol={}&side={}&type={}&newClientOrderId={}",
            timestamp, order.ticker, order.side, order.order_type, order.id
        );
        match (order.order_type, order.side) {
            (OrderType::Market, OrderSide::Buy) => {
                params.push_str(&format!("&quoteOrderQty={}", order.quote_amount));
            }
            (OrderType::Market, OrderSide::Sell) => {
                params.push_str(&format!("&quantity={}", order.amount));
            }
            (OrderType::Limit, _) => {
                params.push_str(&format!("&quantity={}&price={}", order.amount, order.price));
            }
            _ => {}
        }

        let mut mac: Hmac<Sha256> = Hmac::new_from_slice(api_secret.as_bytes())?;
        mac.update(params.as_bytes());
        let signature = encode(mac.finalize().into_bytes());

        let url = format!(
            "{}/api/v3/order?{}&signature={}",
            *ENDPOINT, params, signature
        );

        info!("{}", url);

        let res = self
            .client
            .post(&url)
            .header("X-MBX-APIKEY", api_key)
            .send()
            .await?;

        if !res.status().is_success() {
            let text = &res.text().await?;
            error!("Binance order post failed : {}", text);
            return Err(anyhow::anyhow!("Binance order failed"));
        }

        let res_text = res.text().await?;
        debug!("Binance order response : {}", res_text);

        let order_response: OrderResponse = serde_json::de::from_str(res_text.as_str())?;

        debug!("Binance order response : {:?}", order_response);

        Ok(order_response)
    }
}
