use std::iter::Filter;
use std::slice::Iter;
use std::{cmp::Ordering, time::Duration};

use itertools::Itertools;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};

use crate::order::OrderType;
use crate::{
    marketplace::MarketplaceOrderUpdate,
    order::{Order, OrderSide, OrderStatus},
    portfolio::Portfolio,
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

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Default)]
pub struct OrderListFilters {
    pub ticker: Option<Ticker>,
    pub status: Vec<OrderStatus>,
    pub side: Option<OrderSide>,
    pub session: Option<String>,
    pub strategy: Option<String>,
    pub has_child: Option<bool>,
}

pub struct OrderListSort {
    pub by: OrderListSortBy,
    pub asc: bool,
}

pub enum OrderListSortBy {
    Date,
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

    pub fn add_order(&mut self, order: Order) -> anyhow::Result<Order> {
        if order.status != OrderStatus::Draft {
            anyhow::bail!("Order status is not Draft");
        }

        match (order.side, order.order_type) {
            (OrderSide::Buy, OrderType::Market) => {
                self.portfolio
                    .reserve_funds(&order.ticker.quote, order.quote_amount)?;
            }
            (OrderSide::Buy, OrderType::Limit) => {
                self.portfolio
                    .reserve_funds(&order.ticker.quote, order.amount)?;
            }
            (OrderSide::Sell, OrderType::Market | OrderType::Limit) => {
                self.portfolio
                    .reserve_funds(&order.ticker.base, order.amount)?;
            }
            _ => {
                anyhow::bail!("Order type not supported");
            }
        }

        if let Some(prev_order_id) = &order.prev_order_id {
            if let Some(prev_order) = self.find_by_id(prev_order_id) {
                prev_order.next_order_id = Some(order.id.clone());
            }
        }

        self.orders.push(order.clone());

        Ok(order)
    }

    // remove non active orders older than 2 weeks
    pub fn purge_orders(&mut self, current_time: u64) -> usize {
        let prev_count = self.orders.len();
        self.orders.retain(|order| {
            !matches!(order.status, OrderStatus::Active)
                || Duration::from_millis(current_time.saturating_sub(order.creation_time))
                    > Duration::from_secs(3600 * 24 * 14)
        });
        prev_count - self.orders.len()
    }

    pub fn get_active_sessions(
        &self,
        ticker: &Ticker,
        current_time: u64,
        session_lifetime: &Duration,
    ) -> usize {
        let mut sessions: Vec<String> = self
            .orders
            .iter()
            .filter(|order| {
                order.ticker == *ticker
                    && (Duration::from_millis(current_time.saturating_sub(order.creation_time))
                        < *session_lifetime
                        || (matches!(order.status, OrderStatus::Executed)
                            && order.side == OrderSide::Buy
                            && order.next_order_id.is_none()))
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

    fn get_filter_iterator<'a>(
        &'a self,
        filters: OrderListFilters,
    ) -> impl Iterator<Item = &'a Order> + 'a {
        self.orders.iter().filter(move |order| {
            if let Some(ticker) = &filters.ticker {
                if order.ticker != *ticker {
                    return false;
                }
            }

            if let Some(side) = &filters.side {
                if order.side != *side {
                    return false;
                }
            }

            if !&filters.status.is_empty() {
                if !&filters.status.contains(&order.status) {
                    return false;
                }
            }

            if let Some(has_child) = &filters.has_child {
                if order.next_order_id.is_some() != *has_child {
                    return false;
                }
            }

            return true;
        })
    }

    pub fn find_by(&self, filters: OrderListFilters, sort: OrderListSort) -> Vec<Order> {
        let mut orders: Vec<Order> = self.get_filter_iterator(filters).cloned().collect();
        orders.sort_by(|a, b| match sort.by {
            OrderListSortBy::Date => match (a.working_time, b.working_time) {
                (Some(ts_a), Some(ts_b)) => {
                    if sort.asc {
                        ts_a.cmp(&ts_b)
                    } else {
                        ts_b.cmp(&ts_a)
                    }
                }
                _ => Ordering::Equal,
            },
        });
        orders
    }

    pub fn find_one(&self, filters: OrderListFilters, sort: OrderListSort) -> Option<&Order> {
        let it = self.get_filter_iterator(filters);
        it.min_by(|a, b| match sort.by {
            OrderListSortBy::Date => match (a.working_time, b.working_time) {
                (Some(ts_a), Some(ts_b)) => {
                    if sort.asc {
                        ts_a.cmp(&ts_b)
                    } else {
                        ts_b.cmp(&ts_a)
                    }
                }
                _ => Ordering::Equal,
            },
        })
    }

    pub fn get_last_executed_order_time(&self, filters: OrderListFilters) -> Option<u64> {
        self.find_one(
            filters,
            OrderListSort {
                by: OrderListSortBy::Date,
                asc: false,
            },
        )
        .and_then(|order| order.working_time)
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
