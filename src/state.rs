use std::{cmp::Ordering, time::Duration, usize};

use chrono::Utc;
use itertools::Itertools;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};

use crate::{
    marketplace::MarketplaceOrderUpdate,
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

    pub fn find_by_id(&mut self, id: &String) -> Option<&mut Order> {
        self.orders.iter_mut().find(|o| o.id == *id)
    }

    pub fn update_order(&mut self, update: MarketplaceOrderUpdate) {
        if let Some(existing) = self.find_by_id(&update.client_id) {
            existing.update(update);
        }
    }

    pub fn add_order(&mut self, order: Order) -> Result<Order, String> {
        if order.status != OrderStatus::Draft {
            return Err(format!("Order is not a draft"));
        }

        if self
            .portfolio
            .reserve_funds(
                match order.side {
                    OrderSide::Sell => &order.ticker.base,
                    OrderSide::Buy => &order.ticker.quote,
                },
                match order.side {
                    OrderSide::Sell => order.amount,
                    OrderSide::Buy => order.amount * order.price,
                },
            )
            .is_err()
        {
            return Err(format!("Not enough funds in portfolio"));
        };

        if let Some(prev_order_id) = &order.prev_order_id {
            if let Some(prev_order) = self.find_by_id(&prev_order_id) {
                prev_order.next_order_id = Some(order.id.clone());
            }
        }

        self.orders.push(order.clone());

        Ok(order)
    }

    // remove non active orders older than 2 weeks
    pub fn purge_orders(&mut self, current_time: u64) -> usize {
        let prev_count = self.orders.len();
        self.orders = self
            .orders
            .iter()
            .cloned()
            .filter(|order| {
                !matches!(order.status, OrderStatus::Active)
                    || Duration::from_millis(current_time.saturating_sub(order.creation_time))
                        > Duration::from_secs(3600 * 24 * 14)
            })
            .collect();
        prev_count - self.orders.len()
    }

    pub fn get_active_sessions(&self, current_time: u64, session_lifetime: &Duration) -> usize {
        let mut sessions: Vec<String> = self
            .orders
            .iter()
            .filter(|order| {
                Duration::from_millis(current_time.saturating_sub(order.creation_time))
                    < *session_lifetime
            })
            .flat_map(|order| order.session_id.clone())
            .collect();
        sessions.sort();
        sessions.into_iter().dedup().count()
    }

    pub fn get_session_profit(&self, session_id: &String) -> Decimal {
        self.orders
            .iter()
            .filter(|order| {
                order.session_id.as_ref() == Some(session_id)
                    && matches!(order.status, OrderStatus::Executed)
            })
            .map(|order| match order.side {
                OrderSide::Buy => -order.get_trade_total_price(),
                OrderSide::Sell => order.get_trade_total_price(),
            })
            .sum()
    }

    pub fn get_session_start(&self, session_id: &String) -> Option<u64> {
        self.orders
            .iter()
            .filter(|order| order.session_id.as_ref() == Some(session_id))
            .map(|order| order.creation_time)
            .min()
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
                if let Some(parent_order_price) = order.buy_order_price {
                    return acc
                        + (order.get_trade_total_price() * dec!(0.999) - parent_order_price);
                }
                acc
            })
    }
}
