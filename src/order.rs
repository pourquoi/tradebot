use rust_decimal::Decimal;

use crate::ticker::Ticker;

#[derive(Clone, Debug, PartialEq, Copy)]
pub enum OrderType {
    Buy,
    Sell,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub enum OrderStatus {
    #[default]
    Draft,
    Sent {
        ts: i64,
    },
    Active {
        ts: i64,
    },
    Executed {
        ts: i64,
    },
    Cancelled {
        ts: i64,
    },
}

#[derive(Clone, Debug)]
pub struct Order {
    pub marketplace_id: Option<String>,
    pub order_type: OrderType,
    pub order_status: OrderStatus,
    pub ticker: Ticker,
    pub amount: Decimal,
    pub price: Decimal,
    pub fullfilled: Decimal,
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
