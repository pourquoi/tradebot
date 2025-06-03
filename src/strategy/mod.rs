use chrono::DateTime;
use colored::Colorize;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info};

use crate::{
    marketplace::{CandleEvent, MarketPlaceEvent, TradeEvent},
    order::{Order, OrderType},
    state::State,
    ticker::Ticker,
};

pub mod buy_and_hold;
pub mod scalping;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum StrategyEvent {
    Order(Order),
    Info {
        message: String,
        order: Option<Order>,
    },
    DiscardedEntry {
        ticker: Ticker,
        reason: String,
    },
    DiscardedReentry {
        reason: String,
        order: Order,
    },
    DiscardedSell {
        reason: String,
        order: Order,
    },
}

pub trait Strategy {
    async fn on_trade_event(&mut self, event: &TradeEvent) -> Option<Order>;
    async fn on_candle_event(&mut self, event: &CandleEvent) -> Option<Order>;

    async fn on_marketplace_event(&mut self, event: MarketPlaceEvent, state: Arc<RwLock<State>>) {
        match event {
            MarketPlaceEvent::Trade(event) => {
                if let Some(order) = self.on_trade_event(&event).await {
                    let mut state = state.write().await;
                    info!(
                        "{}",
                        DateTime::from_timestamp_millis(event.trade_time as i64).unwrap()
                    );
                    match state.add_order(order) {
                        Ok(order) => match order.order_type {
                            OrderType::Buy => {
                                info!(
                                    "{} {} {} at {} for {} {}",
                                    "PENDING BUY".green(),
                                    order.amount,
                                    event.ticker.base.green(),
                                    event.price,
                                    order.amount * event.price,
                                    event.ticker.quote
                                );
                                info!("{:?}", order);
                            }
                            OrderType::Sell => {
                                info!(
                                    "{} {} {} at {} for {} {}",
                                    "PENDING SELL".red(),
                                    order.amount,
                                    event.ticker.base.red(),
                                    event.price,
                                    order.amount * event.price,
                                    event.ticker.quote
                                );
                                info!("{:?}", order);
                            }
                        },
                        Err(err) => {
                            error!("Failed adding order : {err:?}");
                        }
                    }
                }
            }
            MarketPlaceEvent::Candle(candle) => {
                self.on_candle_event(&candle).await;
            }
            _ => {}
        }
    }
}
