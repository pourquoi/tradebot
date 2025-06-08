use crate::marketplace::{
    CandleEvent, MarketPlace, MarketPlaceData, MarketPlaceEvent, MarketPlaceSettings, TradeEvent,
};
use crate::order::{Order, OrderSide, OrderStatus, OrderType};
use crate::portfolio::Portfolio;
use crate::state::State;
use crate::strategy::Strategy;
use crate::ticker::Ticker;
use crate::utils::{atr, sma, wsma};
use anyhow::{anyhow, Result};
use chrono::DateTime;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;
use std::usize;
use tokio::sync::RwLock;
use tracing::{debug, info, trace};

use super::StrategyAction;

#[derive(Clone, Debug)]
pub struct ScalpingStrategy<M> {
    marketplace: M,
    tickers: Vec<Ticker>,
    state: Arc<RwLock<State>>,
    trade_event_history: Arc<RwLock<HashMap<Ticker, VecDeque<TradeEvent>>>>,
    candle_event_history: Arc<RwLock<HashMap<Ticker, VecDeque<CandleEvent>>>>,
    target_profit: Decimal,
    quote_amount: Decimal,
    buy_cooldown: Duration,
    initialiazed: bool,
}

impl<M: MarketPlace + MarketPlaceSettings + MarketPlaceData> ScalpingStrategy<M> {
    pub fn new(state: Arc<RwLock<State>>, marketplace: M, tickers: Vec<Ticker>) -> Self {
        let trade_event_history = Arc::from(RwLock::from(HashMap::new()));
        let candle_event_history = Arc::from(RwLock::from(HashMap::new()));
        Self {
            state,
            tickers,
            trade_event_history,
            candle_event_history,
            target_profit: dec!(1.5),
            quote_amount: dec!(300),
            marketplace,
            buy_cooldown: Duration::from_secs(60),
            initialiazed: false,
        }
    }

    async fn init(&mut self, start_time: u64) -> Result<()> {
        for ticker in self.tickers.iter() {
            let candles = self
                .marketplace
                .get_candles(ticker, "1m", None, Some(start_time))
                .await?;
            let mut history = self.candle_event_history.write().await;
            info!(
                "Loaded {} candles for {}. Start={:?} End={:?}",
                candles.len(),
                ticker,
                if candles.len() > 0 {
                    DateTime::from_timestamp_millis(candles[0].start_time as i64)
                } else {
                    None
                },
                if candles.len() > 0 {
                    DateTime::from_timestamp_millis(candles[candles.len() - 1].start_time as i64)
                } else {
                    None
                }
            );
            history.insert(ticker.clone(), candles.into());
        }

        Ok(())
    }

    async fn add_trade_event_history(&mut self, event: TradeEvent) {
        let mut history = self.trade_event_history.write().await;

        let history = history.entry(event.ticker.clone()).or_default();

        history.push_front(event);
        if history.len() > 500 {
            history.pop_back();
        }
    }

    async fn get_sma(&self, ticker: &Ticker, n: usize) -> Option<Decimal> {
        let history = self.candle_event_history.read().await;
        let history = history.get(ticker)?;

        let price_history: Vec<Decimal> = history.iter().map(|event| event.close_price).collect();

        sma(&price_history, n)
    }

    async fn add_candle_event_history(&mut self, event: CandleEvent) {
        let mut history = self.candle_event_history.write().await;

        let history = history.entry(event.ticker.clone()).or_default();

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

        let price_history: Vec<(Decimal, Decimal, Decimal)> = history
            .iter()
            .map(|event| (event.high_price, event.low_price, event.close_price))
            .collect();

        atr(&price_history, n)
    }

    async fn get_wsma(&self, ticker: &Ticker, n: usize) -> Option<Decimal> {
        let history = self.candle_event_history.read().await;
        let history = history.get(ticker)?;

        let price_history: Vec<Decimal> = history.iter().map(|event| event.close_price).collect();

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

    async fn on_trade_event(&mut self, event: &TradeEvent) -> Result<super::StrategyAction> {
        if !self.tickers.contains(&event.ticker) {
            return Ok(StrategyAction::None);
        }

        self.add_trade_event_history(event.clone()).await;

        let sma = self.get_wsma(&event.ticker, 5).await;
        let wsma = self.get_wsma(&event.ticker, 14).await;

        let long_atr = self.get_atr(&event.ticker, 10).await;
        let short_atr = self.get_atr(&event.ticker, 3).await;

        let state = self.state.read().await;

        // if there is a pending order, wait for it to be processed
        if let Some(_) = state.orders.iter().find(|order| {
            order.ticker == event.ticker
                && matches!(
                    order.status,
                    OrderStatus::Draft | OrderStatus::Sent | OrderStatus::Active
                )
        }) {
            return Ok(StrategyAction::Continue {
                ticker: event.ticker.clone(),
                stop_propagation: true,
                reason: "Existing order".to_string(),
            });
        }

        // last order for this ticker
        let last_order = state.get_last_executed_order(&event.ticker, None);

        match last_order {
            Some(last_order) => {
                match (last_order.1.side, last_order.1.status.clone()) {
                    (OrderSide::Buy, OrderStatus::Executed) => {
                        let order = Order {
                            creation_time: event.trade_time,
                            working_time: None,
                            sent_time: None,
                            side: OrderSide::Sell,
                            order_type: OrderType::Market,
                            status: OrderStatus::Draft,
                            ticker: event.ticker.clone(),
                            amount: last_order.1.amount * dec!(0.999), // todo
                            price: event.price,
                            marketplace_id: None,
                            filled_amount: dec!(0),
                            parent_order_price: Some(last_order.1.get_trade_total_price()),
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
                            return Ok(StrategyAction::Continue {
                                ticker: event.ticker.clone(),
                                stop_propagation: true,
                                reason,
                            });
                        }

                        match (sma, wsma) {
                            (Some(sma), Some(wsma)) => {
                                if sma > wsma && take_profit < self.target_profit * dec!(10) {
                                    let reason = format!("Upward trend : skipping sell.");
                                    return Ok(StrategyAction::Continue {
                                        ticker: event.ticker.clone(),
                                        stop_propagation: false,
                                        reason,
                                    });
                                }
                            }
                            _ => {
                                let reason = format!("SMA or WSMA missing");
                                return Ok(StrategyAction::Continue {
                                    ticker: event.ticker.clone(),
                                    stop_propagation: false,
                                    reason,
                                });
                            }
                        }

                        return Ok(StrategyAction::Order { order });
                    }
                    (OrderSide::Sell, OrderStatus::Executed) => {
                        let time_since_sell = match last_order.1.get_last_trade_time() {
                            Some(last_trade_time) if event.trade_time >= last_trade_time => {
                                Some(event.trade_time - last_trade_time)
                            }
                            _ => None,
                        };

                        // too soon for reentry
                        match time_since_sell {
                            Some(time_since_sell)
                                if time_since_sell < self.buy_cooldown.as_millis() as u64 =>
                            {
                                let reason = format!("Too soon for re-entry");
                                return Ok(StrategyAction::Continue {
                                    ticker: event.ticker.clone(),
                                    stop_propagation: true,
                                    reason,
                                });
                            }
                            _ => {}
                        }

                        let last_buy =
                            state.get_last_executed_order(&event.ticker, Some(OrderSide::Buy));
                        let first_buy =
                            state.get_first_executed_order(&event.ticker, Some(OrderSide::Buy));

                        let potential_order = match self.get_desired_buy_amount(
                            &event.ticker,
                            &state.portfolio,
                            event.price,
                        ) {
                            Ok(amount) => {
                                let mut order = Order {
                                    creation_time: event.trade_time,
                                    working_time: None,
                                    sent_time: None,
                                    side: OrderSide::Buy,
                                    order_type: OrderType::Market,
                                    status: OrderStatus::Draft,
                                    ticker: event.ticker.clone(),
                                    amount,
                                    price: event.price,
                                    marketplace_id: None,
                                    filled_amount: dec!(0),
                                    parent_order_price: Some(last_order.1.get_trade_total_price()),
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

                                let skip_condition = match time_since_sell {
                                    Some(time_since_sell) => {
                                        time_since_sell
                                            > Duration::from_secs(1 * 60).as_millis() as u64
                                    }
                                    None => true,
                                };

                                if last_buy.1.price <= event.price
                                    && first_buy.1.price <= event.price
                                    && !skip_condition
                                {
                                    let reason = format!(
                                        "First ({}) and last ({}) buy order price higher than current price : skipping buy.",
                                        first_buy.1.price,
                                        last_buy.1.price
                                    );
                                    return Ok(StrategyAction::Continue {
                                        ticker: event.ticker.clone(),
                                        stop_propagation: false,
                                        reason,
                                    });
                                }

                                if !skip_condition {
                                    match (sma, wsma) {
                                        (Some(sma), Some(wsma)) => {
                                            if sma < wsma {
                                                let reason = format!("SMA < WSMA : skipping buy.");
                                                return Ok(StrategyAction::Continue {
                                                    ticker: event.ticker.clone(),
                                                    stop_propagation: false,
                                                    reason,
                                                });
                                            }
                                            if sma > event.price {
                                                let reason = format!("SMA > price : skipping buy.");
                                                return Ok(StrategyAction::Continue {
                                                    ticker: event.ticker.clone(),
                                                    stop_propagation: false,
                                                    reason,
                                                });
                                            }
                                        }
                                        _ => {
                                            let reason =
                                                format!("SMA or WSMA missing : skipping buy.");
                                            return Ok(StrategyAction::Continue {
                                                ticker: event.ticker.clone(),
                                                stop_propagation: false,
                                                reason,
                                            });
                                        }
                                    }

                                    match (short_atr, long_atr) {
                                        (Some(short_atr), Some(long_atr)) => {
                                            if short_atr < long_atr {
                                                let reason = format!("Average true range lower than usual : skipping buy.");

                                                return Ok(StrategyAction::Continue {
                                                    ticker: event.ticker.clone(),
                                                    stop_propagation: false,
                                                    reason,
                                                });
                                            }
                                        }
                                        _ => {
                                            let reason = format!("ATR missing : skipping buy.");
                                            return Ok(StrategyAction::Continue {
                                                ticker: event.ticker.clone(),
                                                stop_propagation: false,
                                                reason,
                                            });
                                        }
                                    }
                                }

                                let pullback_pct =
                                    (last_order.1.price - event.price) / last_order.1.price;

                                if !skip_condition && pullback_pct < dec!(0.01) {
                                    let reason = format!(
                                        "No significant pullback since last sell : skipping buy."
                                    );
                                    return Ok(StrategyAction::Continue {
                                        ticker: event.ticker.clone(),
                                        stop_propagation: false,
                                        reason,
                                    });
                                }

                                return Ok(StrategyAction::Order {
                                    order: potential_order,
                                });
                            }
                            _ => {}
                        };
                        return Ok(StrategyAction::None);
                    }
                    _ => return Ok(StrategyAction::None),
                }
            }
            None => {
                let amount = self
                    .get_desired_buy_amount(&event.ticker, &state.portfolio, event.price)
                    .map_err(|err| anyhow!("{}", err))?;
                //
                // entry logic
                //
                match (wsma, sma) {
                    (Some(wsma), Some(sma)) => {
                        if wsma > event.price {
                            let reason = format!("SMA > price : skipping entry.");
                            return Ok(StrategyAction::Continue {
                                ticker: event.ticker.clone(),
                                stop_propagation: false,
                                reason,
                            });
                        }
                        if sma < wsma {
                            let reason = format!("SMA < WSMA : skipping entry.");
                            return Ok(StrategyAction::Continue {
                                ticker: event.ticker.clone(),
                                stop_propagation: false,
                                reason,
                            });
                        }
                    }
                    _ => return Ok(StrategyAction::None),
                }

                match (short_atr, long_atr) {
                    (Some(short_atr), Some(long_atr)) => {
                        if short_atr < long_atr {
                            let reason =
                                format!("Average true range lower than usual : skipping entry.");
                            return Ok(StrategyAction::Continue {
                                ticker: event.ticker.clone(),
                                stop_propagation: false,
                                reason,
                            });
                        }
                    }
                    _ => return Ok(StrategyAction::None),
                }

                let mut order = Order {
                    creation_time: event.trade_time,
                    working_time: None,
                    sent_time: None,
                    side: OrderSide::Buy,
                    order_type: OrderType::Market,
                    status: OrderStatus::Draft,
                    ticker: event.ticker.clone(),
                    amount,
                    price: event.price,
                    marketplace_id: None,
                    filled_amount: dec!(0),
                    parent_order_price: None,
                    trades: Vec::new(),
                };

                match self
                    .marketplace
                    .adjust_order_price_and_amount(&mut order)
                    .await
                {
                    Ok(()) => {
                        return Ok(StrategyAction::Order { order });
                    }
                    Err(err) => {
                        return Err(err);
                    }
                };
            }
        }
    }
}

impl<M> Strategy for ScalpingStrategy<M>
where
    M: MarketPlace + MarketPlaceSettings + MarketPlaceData,
{
    async fn on_marketplace_event(
        &mut self,
        event: crate::marketplace::MarketPlaceEvent,
    ) -> Result<super::StrategyAction> {
        match event {
            MarketPlaceEvent::Trade(event) => {
                if self.initialiazed {
                    self.on_trade_event(&event).await
                } else {
                    Ok(StrategyAction::None)
                }
            }
            MarketPlaceEvent::Candle(event) => {
                if !self.initialiazed {
                    self.init(event.start_time).await.unwrap();
                    self.initialiazed = true;
                }

                if !self.tickers.contains(&event.ticker) {
                    return Ok(StrategyAction::Continue {
                        ticker: event.ticker.clone(),
                        stop_propagation: false,
                        reason: "Other ticker".to_string(),
                    });
                }

                self.add_candle_event_history(event.clone()).await;

                Ok(StrategyAction::Continue {
                    ticker: event.ticker.clone(),
                    stop_propagation: false,
                    reason: "None".to_string(),
                })
            }
        }
    }
}
