use anyhow::Result;
use colored::Colorize;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, error, info};

use crate::marketplace::{MarketPlace, MarketPlaceEvent, TradeEvent};
use crate::order::{Order, OrderStatus, OrderType};
use crate::portfolio::Portfolio;
use crate::state::State;
use crate::strategy::Strategy;
use crate::ticker::Ticker;
use crate::utils::short_term_sma;

#[derive(Clone, Debug)]
pub struct ScalpingStrategy<M> {
    marketplace: M,
    tickers: Vec<Ticker>,
    state: Arc<Mutex<State>>,
    history: HashMap<Ticker, Vec<TradeEvent>>,
    target_profit: Decimal,
    quote_amount: Decimal,
}

impl<M: MarketPlace> ScalpingStrategy<M> {
    pub fn new(state: Arc<Mutex<State>>, marketplace: M, tickers: Vec<Ticker>) -> Self {
        let history = HashMap::new();
        Self {
            state,
            tickers,
            history,
            target_profit: dec!(2),
            quote_amount: dec!(300),
            marketplace,
        }
    }

    pub async fn init(&mut self) {
        let _ = self.marketplace.init(self.tickers.clone()).await;
    }

    fn add_history(&mut self, event: TradeEvent) -> &Vec<TradeEvent> {
        let history = self
            .history
            .entry(event.ticker.clone())
            .or_insert(Vec::new());
        history.push(event);
        if history.len() > 500 {
            history.remove(0);
        }
        history
    }

    fn get_desired_buy_amount(
        &self,
        ticker: &Ticker,
        portfolio: &Portfolio,
        current_price: Decimal,
    ) -> Result<Decimal, String> {
        if let Some(asset) = portfolio.assets.get(&ticker.quote) {
            if asset.amount >= self.quote_amount {
                return Ok(self.quote_amount / current_price);
            } else {
                return Err(format!(
                    "Not enough {} in portfolio to buy {} : only got {}.",
                    ticker.quote, self.quote_amount, asset.amount
                ));
            }
        }
        Err(format!("Asset {} not present in portfolio.", ticker.quote))
    }

    async fn on_ticker_price(&mut self, event: &TradeEvent) -> Option<Order> {
        if !self.tickers.contains(&event.ticker) {
            return None;
        }

        let history = self.add_history(event.clone());

        let price_history: Vec<Decimal> = history.iter().map(|event| event.price).collect();
        let sma_20 = short_term_sma(&price_history, 20);

        let state = self.state.lock().await;

        // if there is a pending order, wait for it to be processed
        if let Some(pending_order) = state
            .orders
            .iter()
            .find(|order| order.ticker == event.ticker && order.is_pending())
        {
            debug!(
                "There is already a pending order for {} : skipping.",
                event.ticker
            );
            debug!("{:?}", pending_order);
            return None;
        }

        // last order for this ticker
        let last_order = state.get_last_executed_order(&event.ticker, None);

        match last_order {
            Some(last_order) => match last_order.1.order_type {
                OrderType::Buy => {
                    let order = Order {
                        order_type: OrderType::Sell,
                        order_status: OrderStatus::Draft,
                        ticker: event.ticker.clone(),
                        amount: last_order.1.amount,
                        price: event.price,
                        marketplace_id: None,
                        fullfilled: dec!(0),
                        parent_order: Some(last_order.0),
                    };
                    let receive =
                        last_order.1.amount * event.price * (dec!(1) - M::get_fees(&order));
                    let take_profit = receive - last_order.1.amount * last_order.1.price;
                    // sell if the profit reach the target profit
                    if take_profit >= self.target_profit {
                        return Some(order);
                    } else {
                        debug!(
                            "Profit too low to sell {} {} : missing {} {}.",
                            last_order.1.amount,
                            event.ticker.base,
                            self.target_profit - take_profit,
                            event.ticker.quote
                        );
                    }
                    return None;
                }
                OrderType::Sell => {
                    let last_buy =
                        state.get_last_executed_order(&event.ticker, Some(OrderType::Buy));
                    let first_buy =
                        state.get_first_executed_order(&event.ticker, Some(OrderType::Buy));
                    let potential_order = match self.get_desired_buy_amount(
                        &event.ticker,
                        &state.portfolio,
                        event.price,
                    ) {
                        Ok(amount) => {
                            let mut order = Order {
                                order_type: OrderType::Buy,
                                order_status: OrderStatus::Draft,
                                ticker: event.ticker.clone(),
                                amount,
                                price: event.price,
                                marketplace_id: None,
                                fullfilled: dec!(0),
                                parent_order: Some(last_order.0),
                            };
                            match self.marketplace.adjust_order_price_and_amount(&mut order) {
                                Ok(()) => Some(order),
                                Err(_err) => None,
                            }
                        }
                        Err(_err) => None,
                    };
                    match (potential_order, last_buy, first_buy) {
                        (Some(potential_order), Some(last_buy), Some(first_buy)) => {
                            if last_buy.1.price > event.price || first_buy.1.price > event.price {
                                // buy if price is going down
                                if sma_20.map_or(true, |sma| sma > event.price) {
                                    return Some(potential_order);
                                } else {
                                    debug!("SMA {:?} <= {} : skipping buy.", sma_20, event.price);
                                }
                            } else {
                                debug!(
                                    "First ({}) and last ({}) buy order price higher than current price : skipping buy.",
                                    first_buy.1.price,
                                    last_buy.1.price
                                );
                            }
                        }
                        _ => {}
                    };
                    return None;
                }
            },
            None => {
                // buy when price going down
                if sma_20.map_or(false, |sma| sma > event.price) {
                    match self.get_desired_buy_amount(&event.ticker, &state.portfolio, event.price)
                    {
                        Ok(amount) => {
                            let mut order = Order {
                                order_type: OrderType::Buy,
                                order_status: OrderStatus::Draft,
                                ticker: event.ticker.clone(),
                                amount,
                                price: event.price,
                                marketplace_id: None,
                                fullfilled: dec!(0),
                                parent_order: None,
                            };
                            match self.marketplace.adjust_order_price_and_amount(&mut order) {
                                Ok(()) => {
                                    return Some(order);
                                }
                                Err(_err) => {}
                            };
                        }
                        Err(_err) => {}
                    }
                } else {
                    debug!("SMA {:?} <= {} : skipping buy.", sma_20, event.price);
                }
                return None;
            }
        }
    }
}

impl<M: MarketPlace> Strategy for ScalpingStrategy<M> {
    async fn on_marketplace_event(&mut self, event: MarketPlaceEvent) {
        match event {
            MarketPlaceEvent::Trade(event) => {
                if let Some(order) = self.on_ticker_price(&event).await {
                    let mut state = self.state.lock().await;
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
            _ => {}
        }
    }
}
