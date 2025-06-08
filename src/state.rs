use std::{cmp::Ordering, collections::VecDeque, usize};

use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};

use crate::{
    order::{Order, OrderSide, OrderStatus},
    portfolio::{self, Portfolio},
    ticker::Ticker,
};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct State {
    pub portfolio: Portfolio,
    pub orders: Vec<Order>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum StateEvent {
    Portfolio(Portfolio),
    Orders(Vec<Order>),
}

impl State {
    pub fn new() -> Self {
        Self {
            portfolio: Portfolio::new(),
            orders: vec![],
        }
    }

    pub fn add_order(&mut self, order: Order) -> Result<Order, String> {
        if order.status != OrderStatus::Draft {
            return Err(format!("Order is not a draft"));
        }

        let enough_funds = match order.side {
            OrderSide::Buy => match self.portfolio.assets.get(&order.ticker.quote) {
                Some(asset) => asset.amount >= order.price * order.amount,
                None => false,
            },
            OrderSide::Sell => true,
        };

        if !enough_funds {
            return Err(format!("Not enough funds in portfolio"));
        }

        self.orders.push(order.clone());

        Ok(order)
    }

    pub fn get_first_executed_order(
        &self,
        ticker: &Ticker,
        order_type: Option<OrderSide>,
    ) -> Option<(usize, &Order)> {
        self.orders
            .iter()
            .enumerate()
            .filter(|(_i, order)| {
                order.ticker == *ticker
                    && order_type.is_none_or(|t| t == order.side)
                    && matches!(order.status, OrderStatus::Executed)
            })
            .min_by(|a, b| match (&a.1.working_time, &b.1.working_time) {
                (Some(ts_a), Some(ts_b)) => ts_a.cmp(ts_b),
                _ => Ordering::Equal,
            })
    }

    pub fn get_last_executed_order(
        &self,
        ticker: &Ticker,
        order_type: Option<OrderSide>,
    ) -> Option<(usize, &Order)> {
        self.orders
            .iter()
            .enumerate()
            .filter(|(_i, order)| {
                order.ticker == *ticker
                    && order_type.is_none_or(|t| t == order.side)
                    && matches!(order.status, OrderStatus::Executed)
            })
            .max_by(|a, b| match (&a.1.working_time, &b.1.working_time) {
                (Some(ts_a), Some(ts_b)) => ts_a.cmp(ts_b),
                _ => Ordering::Equal,
            })
    }

    pub fn get_total_scalped(&self, base_asset: String) -> Decimal {
        self.orders
            .iter()
            .filter(|order| {
                matches!(order.side, OrderSide::Sell)
                    && order.ticker.base == base_asset
                    && matches!(order.status, OrderStatus::Executed)
            })
            .fold(dec!(0), |acc, order| {
                if let Some(parent_order_price) = order.parent_order_price {
                    return acc
                        + (order.get_trade_total_price() * dec!(0.999) - parent_order_price);
                }
                acc
            })
    }
}
