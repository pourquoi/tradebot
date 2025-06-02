use rust_decimal_macros::dec;
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};
use tracing::{debug, trace};

use rust_decimal::Decimal;
use tokio::sync::RwLock;

use crate::{
    marketplace::{CandleEvent, MarketPlace, TradeEvent},
    order::{Order, OrderStatus, OrderType},
    portfolio::Portfolio,
    state::State,
    ticker::Ticker,
    utils::{atr, sma, wsma},
};

use super::Strategy;

#[derive(Clone, Debug)]
pub struct BuyAndHoldStrategy<M> {
    marketplace: M,
    tickers: Vec<Ticker>,
    state: Arc<RwLock<State>>,
    trade_event_history: Arc<RwLock<HashMap<Ticker, VecDeque<TradeEvent>>>>,
    candle_event_history: Arc<RwLock<HashMap<Ticker, VecDeque<CandleEvent>>>>,
    quote_amount: Decimal,
}

impl<M: MarketPlace> BuyAndHoldStrategy<M> {
    pub fn new(state: Arc<RwLock<State>>, marketplace: M, tickers: Vec<Ticker>) -> Self {
        let trade_event_history = Arc::from(RwLock::from(HashMap::new()));
        let candle_event_history = Arc::from(RwLock::from(HashMap::new()));
        Self {
            state,
            tickers,
            trade_event_history,
            candle_event_history,
            marketplace,
            quote_amount: dec!(300),
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

impl<M: MarketPlace> Strategy for BuyAndHoldStrategy<M> {
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

        let sma = self.get_sma(&event.ticker, 10).await?;
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
            None => {
                //
                // entry logic
                //
                if sma >= event.price {
                    trace!("SMA > price : skipping entry.");
                    //return None;
                }

                if short_atr <= long_atr {
                    trace!("Average true range lower than usual : skipping entry.");
                    //return None;
                }

                if sma < wsma {
                    trace!("SMA < WSMA : skipping entry.");
                    //return None;
                }

                match self.get_desired_buy_amount(&event.ticker, &state.portfolio, event.price) {
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
            _ => {}
        }

        None
    }
}
