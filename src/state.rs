use std::cmp::Ordering;

use crate::{
    order::{Order, OrderStatus, OrderType},
    portfolio::Portfolio,
    ticker::Ticker,
};

#[derive(Clone, Debug)]
pub struct State {
    pub portfolio: Portfolio,
    pub orders: Vec<Order>,
}

impl State {
    pub fn new() -> Self {
        Self {
            portfolio: Portfolio::new(),
            orders: vec![],
        }
    }

    pub fn add_order(&mut self, order: Order) -> Result<Order, String> {
        if order.order_status != OrderStatus::Draft {
            return Err(format!("can only add draft orders"));
        }

        let enough_funds = match order.order_type {
            OrderType::Buy => match self.portfolio.assets.get(&order.ticker.quote) {
                Some(asset) => asset.amount >= order.price * order.amount,
                None => false,
            },
            OrderType::Sell => true,
        };

        if !enough_funds {
            return Err(format!("unsuficient funds for order {order:?}"));
        }

        self.orders.push(order.clone());

        return Ok(order);
    }

    pub fn get_first_executed_order(
        &self,
        ticker: &Ticker,
        order_type: Option<OrderType>,
    ) -> Option<&Order> {
        self.orders
            .iter()
            .filter(|order| {
                order.ticker == *ticker
                    && order_type.is_none_or(|t| t == order.order_type)
                    && order.is_executed()
            })
            .min_by(|a, b| match (&a.order_status, &b.order_status) {
                (OrderStatus::Executed { ts: ts_a }, OrderStatus::Executed { ts: ts_b }) => {
                    ts_a.cmp(ts_b)
                }
                _ => Ordering::Equal,
            })
    }

    pub fn get_last_executed_order(
        &self,
        ticker: &Ticker,
        order_type: Option<OrderType>,
    ) -> Option<&Order> {
        self.orders
            .iter()
            .filter(|order| {
                order.ticker == *ticker
                    && order_type.is_none_or(|t| t == order.order_type)
                    && order.is_executed()
            })
            .max_by(|a, b| match (&a.order_status, &b.order_status) {
                (OrderStatus::Executed { ts: ts_a }, OrderStatus::Executed { ts: ts_b }) => {
                    ts_a.cmp(ts_b)
                }
                _ => Ordering::Equal,
            })
    }
}
