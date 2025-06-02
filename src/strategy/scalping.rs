use anyhow::Result;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;
use std::usize;
use tokio::sync::RwLock;
use tracing::{debug, info, trace};

use crate::marketplace::{CandleEvent, MarketPlace, TradeEvent};
use crate::order::{Order, OrderStatus, OrderType};
use crate::portfolio::Portfolio;
use crate::state::State;
use crate::strategy::Strategy;
use crate::ticker::Ticker;
use crate::utils::{atr, sma, wsma};

use super::StrategyEvent;

#[derive(Clone, Debug)]
pub struct ScalpingStrategy<M> {
    marketplace: M,
    channel: tokio::sync::broadcast::Sender<StrategyEvent>,
    tickers: Vec<Ticker>,
    state: Arc<RwLock<State>>,
    trade_event_history: Arc<RwLock<HashMap<Ticker, VecDeque<TradeEvent>>>>,
    candle_event_history: Arc<RwLock<HashMap<Ticker, VecDeque<CandleEvent>>>>,
    target_profit: Decimal,
    quote_amount: Decimal,
    buy_cooldown: Duration,
}

impl<M: MarketPlace> ScalpingStrategy<M> {
    pub fn new(
        state: Arc<RwLock<State>>,
        marketplace: M,
        tickers: Vec<Ticker>,
        channel: tokio::sync::broadcast::Sender<StrategyEvent>,
    ) -> Self {
        let trade_event_history = Arc::from(RwLock::from(HashMap::new()));
        let candle_event_history = Arc::from(RwLock::from(HashMap::new()));
        Self {
            state,
            tickers,
            trade_event_history,
            candle_event_history,
            target_profit: dec!(1),
            quote_amount: dec!(300),
            marketplace,
            buy_cooldown: Duration::from_secs(60),
            channel,
        }
    }

    async fn add_trade_event_history(&mut self, event: TradeEvent) {
        let mut history = self.trade_event_history.write().await;

        let history = history
            .entry(event.ticker.clone())
            .or_insert_with(|| VecDeque::with_capacity(500));
        history.push_front(event);
        if history.len() >= 500 {
            history.pop_back();
        }
    }

    async fn get_sma(&self, ticker: &Ticker, n: usize) -> Option<Decimal> {
        let history = self.candle_event_history.read().await;
        let history = history.get(ticker)?;

        let price_history: Vec<Decimal> = history
            .iter()
            .filter_map(|event| event.close_price)
            .collect();

        sma(&price_history, n)
        //
        //let history = self.trade_event_history.read().await;
        //let history = history.get(ticker)?;
        //
        //let price_history: Vec<Decimal> = history.iter().map(|event| event.price).collect();
        //sma(&price_history, n)
    }

    async fn add_candle_event_history(&mut self, event: CandleEvent) {
        let mut history = self.candle_event_history.write().await;
        debug!("{:?}", event);

        let history = history
            .entry(event.ticker.clone())
            .or_insert_with(|| VecDeque::with_capacity(20));

        debug!("candle count for {} : {}", event.ticker, history.len());

        if let Some(last) = history.pop_front() {
            if last.start_time != event.start_time {
                history.push_front(last);
            }
        }
        history.push_front(event);

        if history.len() >= 20 {
            history.pop_back();
        }
    }

    async fn get_atr(&self, ticker: &Ticker, n: usize) -> Option<Decimal> {
        let history = self.candle_event_history.read().await;
        let history = history.get(ticker)?;

        let price_history: Vec<(Decimal, Decimal, Option<Decimal>)> = history
            .iter()
            .map(|event| (event.high_price, event.low_price, event.close_price))
            .collect();

        atr(&price_history, n)
    }

    async fn get_wsma(&self, ticker: &Ticker, n: usize) -> Option<Decimal> {
        let history = self.candle_event_history.read().await;
        let history = history.get(ticker)?;

        let price_history: Vec<Decimal> = history
            .iter()
            .filter_map(|event| event.close_price)
            .collect();

        wsma(&price_history, n)
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
}

impl<M: MarketPlace> Strategy for ScalpingStrategy<M> {
    async fn on_candle_event(&mut self, event: &CandleEvent) -> Option<Order> {
        if !self.tickers.contains(&event.ticker) {
            return None;
        }

        self.add_candle_event_history(event.clone()).await;

        None
    }

    async fn on_trade_event(&mut self, event: &TradeEvent) -> Option<Order> {
        if !self.tickers.contains(&event.ticker) {
            return None;
        }

        self.add_trade_event_history(event.clone()).await;

        let sma = self.get_sma(&event.ticker, 5).await?;
        let wsma = self.get_wsma(&event.ticker, 14).await?;

        let long_atr = self.get_atr(&event.ticker, 10).await?;
        let short_atr = self.get_atr(&event.ticker, 3).await?;

        let state = self.state.read().await;

        // if there is a pending order, wait for it to be processed
        if let Some(pending_order) = state
            .orders
            .iter()
            .find(|order| order.ticker == event.ticker && order.is_pending())
        {
            trace!(
                "There is already a pending order for {} : skipping.",
                event.ticker
            );
            trace!("{:?}", pending_order);
            return None;
        }

        // last order for this ticker
        let last_order = state.get_last_executed_order(&event.ticker, None);

        match last_order {
            Some(last_order) => {
                match (last_order.1.order_type, last_order.1.order_status.clone()) {
                    (OrderType::Buy, OrderStatus::Executed { ts: executed_at }) => {
                        let order = Order {
                            order_type: OrderType::Sell,
                            order_status: OrderStatus::Draft,
                            ticker: event.ticker.clone(),
                            amount: last_order.1.amount,
                            price: event.price,
                            marketplace_id: None,
                            fullfilled: dec!(0),
                            parent_order: Some(last_order.0),
                            trades: Vec::new(),
                        };
                        let fees = self.marketplace.get_fees(&order).await;
                        let receive = last_order.1.amount * event.price * (dec!(1) - fees);
                        let take_profit = receive - last_order.1.amount * last_order.1.price;

                        //
                        // sell logic
                        //

                        if take_profit < self.target_profit {
                            let reason = format!(
                                "Profit too low to sell {} {} : missing {} {}.",
                                last_order.1.amount,
                                event.ticker.base,
                                self.target_profit - take_profit,
                                event.ticker.quote
                            );
                            self.channel.send(StrategyEvent::DiscardedSell {
                                reason,
                                order: last_order.1.clone(),
                            });
                            return None;
                        }

                        if sma > wsma
                            && event.price > sma
                            && take_profit < self.target_profit * dec!(2)
                        {
                            let reason = format!(
                                "SMA {} > WSMA {} or price {} > SMA : skipping sell.",
                                sma, wsma, event.price
                            );
                            self.channel.send(StrategyEvent::DiscardedSell {
                                reason,
                                order: last_order.1.clone(),
                            });
                            return None;
                        }

                        return Some(order);
                    }
                    (OrderType::Sell, OrderStatus::Executed { ts: executed_at }) => {
                        // too soon for reentry
                        if event.trade_time <= executed_at
                            || event.trade_time - executed_at < self.buy_cooldown.as_millis() as u64
                        {
                            trace!("Too soon for re-entry");
                            return None;
                        }

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
                                    trades: Vec::new(),
                                };
                                match self
                                    .marketplace
                                    .adjust_order_price_and_amount(&mut order)
                                    .await
                                {
                                    Ok(()) => Some(order),
                                    Err(_err) => None,
                                }
                            }
                            Err(_err) => None,
                        };

                        match (potential_order, last_buy, first_buy) {
                            (Some(potential_order), Some(last_buy), Some(first_buy)) => {
                                //
                                // reentry logic
                                //

                                if last_buy.1.price <= event.price
                                    && first_buy.1.price <= event.price
                                {
                                    let reason = format!(
                                        "First ({}) and last ({}) buy order price higher than current price : skipping buy.",
                                        first_buy.1.price,
                                        last_buy.1.price
                                    );
                                    self.channel.send(StrategyEvent::DiscardedSell {
                                        reason,
                                        order: last_order.1.clone(),
                                    });
                                    return None;
                                }

                                if sma < wsma {
                                    let message = format!("SMA < WSMA : skipping buy.");
                                    self.channel.send(StrategyEvent::Info {
                                        message,
                                        order: Some(last_order.1.clone()),
                                    });
                                    //return None;
                                }

                                if short_atr <= long_atr {
                                    let message = format!(
                                        "Average true range lower than usual : skipping buy."
                                    );
                                    self.channel.send(StrategyEvent::Info {
                                        message,
                                        order: Some(last_order.1.clone()),
                                    });
                                    //return None;
                                }

                                let pullback_pct =
                                    (last_order.1.price - event.price) / last_order.1.price;

                                if pullback_pct < dec!(0.01) {
                                    let message = format!(
                                        "No significant pullback since last sell : skipping buy."
                                    );
                                    self.channel.send(StrategyEvent::Info {
                                        message,
                                        order: Some(last_order.1.clone()),
                                    });
                                    //return None;
                                }

                                if sma >= event.price {
                                    let message = format!("SMA > price : skipping buy.");
                                    self.channel.send(StrategyEvent::Info {
                                        message,
                                        order: Some(last_order.1.clone()),
                                    });
                                    //return None;
                                }

                                return Some(potential_order);
                            }
                            _ => {}
                        };
                        return None;
                    }
                    _ => return None,
                }
            }
            None => {
                match self.get_desired_buy_amount(&event.ticker, &state.portfolio, event.price) {
                    Ok(amount) => {
                        //
                        // entry logic
                        //

                        if sma >= event.price {
                            let message = format!("SMA > price : skipping entry.");
                            self.channel.send(StrategyEvent::Info {
                                message,
                                order: None,
                            });
                            //return None;
                        }

                        if short_atr <= long_atr {
                            let message =
                                format!("Average true range lower than usual : skipping entry.");
                            self.channel.send(StrategyEvent::Info {
                                message,
                                order: None,
                            });
                            //return None;
                        }

                        if sma < wsma {
                            let message = format!("SMA < WSMA : skipping entry.");
                            self.channel.send(StrategyEvent::Info {
                                message,
                                order: None,
                            });
                            //return None;
                        }

                        let mut order = Order {
                            order_type: OrderType::Buy,
                            order_status: OrderStatus::Draft,
                            ticker: event.ticker.clone(),
                            amount,
                            price: event.price,
                            marketplace_id: None,
                            fullfilled: dec!(0),
                            parent_order: None,
                            trades: Vec::new(),
                        };

                        match self
                            .marketplace
                            .adjust_order_price_and_amount(&mut order)
                            .await
                        {
                            Ok(()) => {
                                return Some(order);
                            }
                            Err(_err) => {}
                        };
                    }
                    Err(_err) => {}
                }
                return None;
            }
        }
    }
}
