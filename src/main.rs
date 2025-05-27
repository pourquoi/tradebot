use chrono::prelude::*;
use clap::Parser;
use colored::Colorize;
use futures::future;
use marketplace::binance::Binance;
use marketplace::{MarketPlace, MarketPlaceEvent};
use order::{OrderStatus, OrderType};
use portfolio::Asset;
use rust_decimal_macros::dec;
use state::State;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use strategy::{scalping::ScalpingStrategy, Strategy};
use ticker::Ticker;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tracing::{debug, error, info};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

mod marketplace;
mod order;
mod portfolio;
mod state;
mod strategy;
mod ticker;
mod utils;

#[derive(Parser, Debug)]
struct Args {
    #[arg(long)]
    replay_path: Option<PathBuf>,
    #[arg(long)]
    store_path: Option<PathBuf>,
}

#[tokio::main]
async fn main() {
    // fmt tracing subscriber -> log to stdout
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();

    let args = Args::parse();

    let tickers = vec![
        Ticker::new("BTC", "USDT"),
        Ticker::new("ETH", "USDT"),
        Ticker::new("BNB", "USDT"),
        Ticker::new("XNO", "USDT"),
    ];

    let state: Arc<Mutex<state::State>> = Arc::from(Mutex::from(state::State::new()));

    {
        let state = state.clone();
        let mut state = state.lock().await;
        state.portfolio.assets.insert(
            String::from("USDT"),
            Asset {
                amount: dec!(2000),
                symbol: String::from("USDT").to_uppercase(),
                value: None,
            },
        );
    }

    let mut binance = Binance::new();
    binance.ping().await.expect("Failed to ping binance");

    let (tx, mut rx) = tokio::sync::broadcast::channel::<marketplace::MarketPlaceEvent>(64);

    // spawn strategy task
    let strategy_task = tokio::task::spawn({
        let state = state.clone();
        let tickers = tickers.clone();
        let tx = tx.clone();
        let mut rx = tx.subscribe();
        async move {
            let mut strategy = ScalpingStrategy::new(state, Binance::new(), tickers);
            strategy.init().await;
            loop {
                // wait for a marketplace event
                let event = rx.recv().await;
                if let Ok(event) = event {
                    strategy.on_marketplace_event(event).await;
                }
            }
        }
    });

    // spawn marketplace task
    let marketplace_task = tokio::task::spawn({
        let tickers = tickers.clone();
        let tx = tx.clone();

        async move {
            if let Some(path) = args.replay_path {
                // todo
                // load events form file and send them on tx
                let file = File::open(path).await.expect("Failed to open replay file");
                let reader = BufReader::new(file);
                let mut lines = reader.lines();

                while let Ok(Some(line)) = lines.next_line().await {
                    if let Ok(event) = serde_json::de::from_str(&line) {
                        tx.send(event).unwrap();
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                }
            } else {
                binance.start(tickers, tx).await;
            }
        }
    });

    // save marketplace events for backtesting
    let store_events_task = match args.store_path {
        Some(path) => {
            tokio::task::spawn({
                let tx = tx.clone();
                let mut rx = tx.subscribe();

                async move {
                    let mut file = OpenOptions::new()
                        .create(true)
                        .write(true)
                        .open(path)
                        .await
                        .expect("Failed to open file for storing events");

                    loop {
                        // wait for a marketplace event
                        let event = rx.recv().await;
                        if let Ok(event) = event {
                            match serde_json::ser::to_string(&event) {
                                Ok(json) => {
                                    file.write_all(json.as_bytes())
                                        .await
                                        .expect("Failed to write to store file");
                                    file.write_all(b"\n")
                                        .await
                                        .expect("Failed to write to store file");
                                }
                                Err(err) => {
                                    error!("Failed to save event");
                                }
                            }
                        }
                    }
                }
            })
        }
        None => tokio::task::spawn({
            let future = future::pending();
            async move {
                let () = future.await;
            }
        }),
    };

    // spawn pending orders processing task
    let pending_orders_task = tokio::task::spawn({
        let state = state.clone();
        async move {
            loop {
                let mut state = state.lock().await;
                let State { orders, .. } = &mut *state;
                for order in orders
                    .iter_mut()
                    .filter(|order| order.order_status == OrderStatus::Draft)
                {
                    // simulate sending request to markeplace
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    order.order_status = OrderStatus::Sent {
                        ts: Utc::now().timestamp_millis(),
                    };
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    order.order_status = OrderStatus::Active {
                        ts: Utc::now().timestamp_millis(),
                    }
                }
            }
        }
    });

    // spawn orders results task
    let orders_result_task = tokio::task::spawn({
        let state = state.clone();
        let tx = tx.clone();
        let mut rx = tx.subscribe();

        async move {
            loop {
                let event = rx.recv().await;
                if let Ok(event) = event {
                    match event {
                        MarketPlaceEvent::Trade(event) => {
                            let mut state = state.lock().await;

                            let State {
                                portfolio, orders, ..
                            } = &mut *state;

                            // find active order (not fullfilled)
                            for order in orders.iter_mut().filter(|order| {
                                matches!(order.order_status, OrderStatus::Active { .. })
                                    && order.ticker == event.ticker
                            }) {
                                // simulate if a trade will eat part of the order
                                let trade_amount = match order.order_type {
                                    // sell order : if the trade price >= order price, it will be eaten
                                    OrderType::Sell => {
                                        if event.price >= order.price {
                                            Some(event.quantity)
                                        } else {
                                            None
                                        }
                                    }
                                    // buy order : if the trade price <= order price, it will be eaten
                                    OrderType::Buy => {
                                        if event.price <= order.price {
                                            Some(event.quantity)
                                        } else {
                                            None
                                        }
                                    }
                                };

                                if trade_amount.is_none() {
                                    debug!("No trade for order");
                                    continue;
                                }
                                let trade_amount = trade_amount.unwrap();

                                // simulate a marketplace request
                                tokio::time::sleep(Duration::from_secs(1)).await;

                                let to_fullfill = order.amount - order.fullfilled;
                                let traded_amount = if to_fullfill > trade_amount {
                                    order.fullfilled += trade_amount;
                                    info!(
                                        "{} for order {} {}/{} : +{}",
                                        match order.order_type {
                                            OrderType::Buy => "PARTIAL BUY".green(),
                                            OrderType::Sell => "PARTIAL SELL".green(),
                                        },
                                        Ticker::to_string(&event.ticker),
                                        order.fullfilled,
                                        order.amount,
                                        trade_amount
                                    );
                                    trade_amount
                                } else {
                                    order.order_status = OrderStatus::Executed {
                                        ts: Utc::now().timestamp_millis(),
                                    };
                                    order.fullfilled = order.amount;
                                    info!(
                                        "{} for order {} {}/{} : +{}",
                                        match order.order_type {
                                            OrderType::Buy => "FINAL BUY".green(),
                                            OrderType::Sell => "FINAL SELL".green(),
                                        },
                                        Ticker::to_string(&event.ticker),
                                        order.fullfilled,
                                        order.amount,
                                        to_fullfill
                                    );
                                    to_fullfill
                                };

                                match order.order_type {
                                    OrderType::Sell => {
                                        // increase portfolio quote asset
                                        let added_quote_amount = order.price
                                            * traded_amount
                                            * (dec!(1) - Binance::get_fees(order));
                                        portfolio
                                            .assets
                                            .entry(order.ticker.quote.clone())
                                            .and_modify(|asset| {
                                                asset.amount += added_quote_amount;
                                                asset.value = Some(
                                                    asset.value.unwrap_or(dec!(0))
                                                        + added_quote_amount,
                                                );
                                            })
                                            .or_insert(Asset {
                                                symbol: order.ticker.quote.clone(),
                                                amount: added_quote_amount,
                                                value: Some(added_quote_amount),
                                            });

                                        // decrease portfolio base asset
                                        // todo : check if fees apply
                                        let removed_base_amount =
                                            traded_amount * (dec!(1) - Binance::get_fees(order));
                                        portfolio
                                            .assets
                                            .entry(order.ticker.base.clone())
                                            .and_modify(|asset| {
                                                asset.amount -= removed_base_amount;
                                                asset.value = Some(asset.amount * event.price);
                                            })
                                            .or_insert(Asset {
                                                symbol: order.ticker.base.clone(),
                                                amount: -removed_base_amount,
                                                value: Some(-removed_base_amount * event.price),
                                            });
                                    }
                                    OrderType::Buy => {
                                        // increase portfolio base asset
                                        let added_base_amount =
                                            traded_amount * (dec!(1) - Binance::get_fees(order));
                                        portfolio
                                            .assets
                                            .entry(order.ticker.base.clone())
                                            .and_modify(|asset| {
                                                asset.amount += added_base_amount;
                                                asset.value = Some(asset.amount * event.price);
                                            })
                                            .or_insert(Asset {
                                                symbol: order.ticker.base.clone(),
                                                amount: added_base_amount,
                                                value: Some(added_base_amount * event.price),
                                            });

                                        // decrease portfolio quote asset
                                        // no fees, they apply to the bought asset
                                        let removed_quote_amount = event.price * traded_amount;
                                        portfolio
                                            .assets
                                            .entry(order.ticker.quote.clone())
                                            .and_modify(|asset| {
                                                asset.amount -= removed_quote_amount;
                                                asset.value = Some(asset.amount);
                                            })
                                            .or_insert(Asset {
                                                symbol: order.ticker.quote.clone(),
                                                amount: -removed_quote_amount,
                                                value: Some(-removed_quote_amount),
                                            });
                                    }
                                }
                                info!("{}", portfolio);
                            }
                        }
                        _ => {}
                    }
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    });

    let update_portfolio_total_task = tokio::task::spawn({
        let state = state.clone();
        let tx = tx.clone();
        let mut rx = tx.subscribe();
        async move {
            loop {
                let event = rx.recv().await;
                if let Ok(event) = event {
                    match event {
                        marketplace::MarketPlaceEvent::Trade(event) => {
                            let mut state = state.lock().await;
                            if let Some(asset) = state.portfolio.assets.get_mut(&event.ticker.base)
                            {
                                asset.value = Some(asset.amount * event.price);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    });

    let overview_task = tokio::task::spawn({
        let state = state.clone();
        async move {
            loop {
                {
                    let state = state.lock().await;
                    info!("overview {}", state.portfolio);
                }
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
        }
    });

    info!("{}", "STARTING BOT".green());

    tokio::select! {
        _ = marketplace_task => {}
        _ = strategy_task => {}
        _ = pending_orders_task => {}
        _ = orders_result_task => {}
        _ = store_events_task => {}
        _ = update_portfolio_total_task => {}
        _ = overview_task => {}
    }
}
