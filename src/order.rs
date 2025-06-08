use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::ticker::Ticker;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Copy, strum_macros::Display)]
pub enum OrderSide {
    Buy,
    Sell,
}

impl TryFrom<&str> for OrderSide {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value.to_uppercase().as_str() {
            "BUY" => Ok(OrderSide::Buy),
            "SELL" => Ok(OrderSide::Sell),
            other => Err(format!("Unknown order side {}", other)),
        }
    }
}

impl TryFrom<&String> for OrderSide {
    type Error = String;

    fn try_from(value: &String) -> Result<Self, Self::Error> {
        OrderSide::try_from(value.as_str())
    }
}

impl TryFrom<String> for OrderSide {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        OrderSide::try_from(&value)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Copy, strum_macros::Display)]
pub enum OrderType {
    Market,
    Limit,
    StopLoss,
    StopLossLimit,
    TakeProfit,
    TakeProfitLimit,
    LimitMaker,
}

impl TryFrom<&str> for OrderType {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value.to_uppercase().as_str() {
            "LIMIT" => Ok(OrderType::Limit),
            "MARKET" => Ok(OrderType::Market),
            "STOP_LOSS" => Ok(OrderType::StopLoss),
            "STOP_LOSS_LIMIT" => Ok(OrderType::StopLossLimit),
            "TAKE_PROFIT" => Ok(OrderType::TakeProfit),
            "TAKE_PROFIT_LIMIT" => Ok(OrderType::TakeProfitLimit),
            "LIMIT_MAKER" => Ok(OrderType::LimitMaker),
            other => Err(format!("Unknown order type {}", other)),
        }
    }
}

impl TryFrom<&String> for OrderType {
    type Error = String;

    fn try_from(value: &String) -> Result<Self, Self::Error> {
        OrderType::try_from(value.as_str())
    }
}

impl TryFrom<String> for OrderType {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        OrderType::try_from(&value)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default, strum_macros::Display)]
pub enum OrderStatus {
    #[default]
    Draft,
    Sent,
    Active,
    Executed,
    PendingCancel,
    Cancelled,
    Rejected,
    Expired,
}

impl TryFrom<&str> for OrderStatus {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "PENDING_NEW" => Ok(OrderStatus::Sent),
            "NEW" => Ok(OrderStatus::Active),
            "PARTIALLY_FILLED" => Ok(OrderStatus::Active),
            "FILLED" => Ok(OrderStatus::Executed),
            "CANCELED" => Ok(OrderStatus::Cancelled),
            "REJECTED" => Ok(OrderStatus::Rejected),
            "EXPIRED" | "EXPIRED_IN_MATCH" => Ok(OrderStatus::Expired),
            status => Err(format!("Unknown order status {}", status)),
        }
    }
}

impl TryFrom<&String> for OrderStatus {
    type Error = String;

    fn try_from(value: &String) -> Result<Self, Self::Error> {
        OrderStatus::try_from(value.as_str())
    }
}

impl TryFrom<String> for OrderStatus {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        OrderStatus::try_from(&value)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct OrderTrade {
    pub trade_time: u64,
    pub amount: Decimal,
    pub price: Decimal,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Order {
    pub creation_time: u64,
    pub sent_time: Option<u64>,
    pub working_time: Option<u64>,
    pub ticker: Ticker,
    pub side: OrderSide,
    pub order_type: OrderType,
    pub status: OrderStatus,
    pub amount: Decimal,
    pub price: Decimal,
    pub filled_amount: Decimal,
    pub trades: Vec<OrderTrade>,
    pub parent_order_price: Option<Decimal>,
    pub marketplace_id: Option<String>,
}

impl Order {
    pub fn get_last_trade_time(&self) -> Option<u64> {
        self.trades.iter().map(|trade| trade.trade_time).max()
    }
}
