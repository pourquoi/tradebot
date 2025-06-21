use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{marketplace::MarketplaceOrderUpdate, ticker::Ticker};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Copy, strum_macros::Display)]
pub enum OrderSide {
    #[strum(serialize = "BUY")]
    Buy,
    #[strum(serialize = "SELL")]
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
    #[strum(serialize = "MARKET")]
    Market,
    #[strum(serialize = "LIMIT")]
    Limit,
    #[strum(serialize = "STOP_LOSS")]
    StopLoss,
    #[strum(serialize = "STOP_LOSS_LIMIT")]
    StopLossLimit,
    #[strum(serialize = "TAKE_PROFIT")]
    TakeProfit,
    #[strum(serialize = "TAKE_PROFIT_LIMIT")]
    TakeProfitLimit,
    #[strum(serialize = "LIMIT_MAKER")]
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
    pub id: String,
    pub trade_time: u64,
    pub amount: Decimal,
    pub price: Decimal,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Order {
    pub id: String,
    pub marketplace_id: Option<String>,

    pub creation_time: u64,
    pub working_time: Option<u64>,

    pub ticker: Ticker,
    pub side: OrderSide,
    pub order_type: OrderType,
    pub status: OrderStatus,

    pub amount: Decimal,
    pub quote_amount: Decimal, // for MARKET BUY orders

    pub fees: Decimal,
    pub price: Decimal, // for LIMIT orders

    pub filled_amount: Decimal,
    pub cumulative_quote_amount: Decimal, // actual quote amount spent for a BUY or received for a SELL

    pub trades: Vec<OrderTrade>,

    pub sell_order_price: Option<Decimal>,
    pub buy_order_price: Option<Decimal>,

    pub session_id: Option<String>,

    pub next_order_id: Option<String>,
    pub prev_order_id: Option<String>,

    pub profit: Decimal,
}

impl Order {
    pub fn get_order_base_price(&self) -> Decimal {
        if self.filled_amount > dec!(0) {
            self.cumulative_quote_amount / self.filled_amount
        } else {
            self.price
        }
    }
    
    pub fn new_buy(
        ticker: Ticker,
        amount: Decimal,
        price: Decimal,
        quote_amount: Decimal,
        creation_time: u64,
        sell_order: Option<&Order>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            creation_time,
            fees: dec!(0.001),
            profit: dec!(0),
            working_time: None,
            side: OrderSide::Buy,
            order_type: OrderType::Market,
            status: OrderStatus::Draft,
            ticker,
            amount,
            quote_amount,
            cumulative_quote_amount: dec!(0),
            price,
            marketplace_id: None,
            filled_amount: dec!(0),
            buy_order_price: None,
            sell_order_price: sell_order.map(|sell_order| sell_order.get_trade_total_price()),
            trades: Vec::new(),
            session_id: sell_order
                .map(|sell_order| sell_order.session_id.clone())
                .unwrap_or(Some(Uuid::new_v4().to_string())),
            next_order_id: None,
            prev_order_id: sell_order.map(|sell_order| sell_order.id.clone()),
        }
    }

    pub fn new_sell(
        ticker: Ticker,
        amount: Decimal,
        price: Decimal,
        creation_time: u64,
        buy_order: Option<&Order>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            creation_time,
            working_time: None,
            fees: dec!(0.001),
            profit: dec!(0),
            side: OrderSide::Sell,
            order_type: OrderType::Market,
            status: OrderStatus::Draft,
            ticker,
            amount,
            quote_amount: dec!(0), // todo
            cumulative_quote_amount: dec!(0),
            price,
            marketplace_id: None,
            filled_amount: dec!(0),
            buy_order_price: buy_order.map(|buy_order| buy_order.get_trade_total_price()),
            sell_order_price: None,
            trades: Vec::new(),
            session_id: buy_order.and_then(|buy_order| buy_order.session_id.clone()),
            next_order_id: None,
            prev_order_id: buy_order.map(|buy_order| buy_order.id.clone()),
        }
    }

    pub fn update(&mut self, update: MarketplaceOrderUpdate) {
        self.marketplace_id = Some(update.marketplace_id);
        self.working_time = update.working_time;
        self.status = update.status;

        if let Some(trade) = update.trade {
            if !self.trades.iter().any(|t| t.id == trade.id) {
                self.trades.push(trade.clone());
            }
        }
    }

    pub fn get_last_trade_time(&self) -> Option<u64> {
        self.trades.iter().map(|trade| trade.trade_time).max()
    }

    pub fn get_trade_total_price(&self) -> Decimal {
        self.trades
            .iter()
            .map(|trade| trade.amount * trade.price)
            .sum::<Decimal>()
    }
}
