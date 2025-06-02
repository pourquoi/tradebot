use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::ticker::Ticker;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Copy)]
pub enum OrderType {
    Buy,
    Sell,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub enum OrderStatus {
    #[default]
    Draft,
    Sent {
        ts: u64,
    },
    Active {
        ts: u64,
    },
    Executed {
        ts: u64,
    },
    Cancelled {
        ts: u64,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Order {
    pub marketplace_id: Option<String>,
    pub order_type: OrderType,
    pub order_status: OrderStatus,
    pub ticker: Ticker,
    pub amount: Decimal,
    pub price: Decimal,
    pub fullfilled: Decimal,
    pub parent_order: Option<usize>,
    pub trades: Vec<OrderTrade>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct OrderTrade {
    pub trade_time: u64,
    pub amount: Decimal,
    pub price: Decimal,
}

impl Order {
    pub fn is_pending(&self) -> bool {
        matches!(
            self.order_status,
            OrderStatus::Draft | OrderStatus::Sent { .. } | OrderStatus::Active { .. }
        )
    }

    pub fn is_executed(&self) -> bool {
        matches!(self.order_status, OrderStatus::Executed { .. })
    }
}
