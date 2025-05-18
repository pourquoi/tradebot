use marketplace::MarketPlace;
use marketplace::{binance::Binance, MarketPlaceEvent};
use rust_decimal_macros::dec;
use std::sync::Arc;
use std::time::Duration;
use strategy::{scalping::ScalpingStrategy, Strategy};
use tokio::sync::Mutex;
use tracing::{error, info};

mod marketplace;
mod portfolio;
mod state;
mod strategy;
mod utils;

#[tokio::main]
async fn main() {
    // fmt tracing subscriber -> log to stdout
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    let binance = Binance::new();
    binance.ping().await.expect("Failed to ping binance");

    let strategy = ScalpingStrategy {};

    let (tx, mut rx) = tokio::sync::broadcast::channel::<marketplace::MarketPlaceEvent>(64);

    // spawn strategy task
    let strategy2 = strategy.clone();
    let strategy_task = tokio::task::spawn(async move {
        loop {
            // wait for a marketplace event
            let message = rx.recv().await;
            match message {
                Ok(MarketPlaceEvent::TickerPrice(price_event)) => {
                    strategy2.on_ticker_price(price_event);
                }
                _ => {}
            }
        }
    });

    // spawn marketplace task
    let marketplace_task = tokio::task::spawn(async move {
        binance
            .start(vec![String::from("btcusdt"), String::from("bnbusdt")], tx)
            .await;
    });

    tokio::select! {
        _ = marketplace_task => {}
        _ = strategy_task => {}
    }

    //let state: Arc<Mutex<algo::State>> = Arc::from(Mutex::from(algo::State::new()));
    //
    //let client = binance::new_client();
    //
    //binance::ping(&client)
    //    .await
    //    .expect("Failed to ping binance api");
    //
    //let (tx, mut rx) =
    //    tokio::sync::broadcast::channel::<binance::MultiStream<binance::KLineStream>>(64);
    //
    //let state2 = state.clone();
    //let algo_task = tokio::task::spawn(async move {
    //    loop {
    //        let message = rx.recv().await;
    //        match message {
    //            Ok(message) => {
    //                let tick = Tick {
    //                    symbol: message.data.symbol.clone(),
    //                    price: (message.data.data.high_price + message.data.data.low_price)
    //                        / dec!(2),
    //                };
    //                let mut lock = state2.lock().await;
    //                algo::tick(&mut lock, tick);
    //            }
    //            Err(err) => {
    //                error!("Channel error: {:?}", err);
    //            }
    //        }
    //    }
    //});
    //
    //let state3 = state.clone();
    //let orders_task = tokio::task::spawn(async move {
    //    loop {
    //        tokio::time::sleep(Duration::from_secs(rand::random_range::<u64, _>(1..5))).await;
    //        let mut lock = state3.lock().await;
    //        let orders = lock.orders.clone();
    //        for (symbol, order) in orders.into_iter() {
    //            match order.order_type {
    //                algo::OrderType::Sell => {
    //                    info!("-- sell order fullfilled for {} --", symbol);
    //                    lock.bank = lock.bank + order.price;
    //                    lock.order_history.insert(symbol.clone(), order.clone());
    //                    lock.orders.remove(&symbol);
    //                }
    //                algo::OrderType::Buy => {
    //                    info!("-- buy order fullfilled for {} --", symbol);
    //                    lock.positions.insert(
    //                        symbol.clone(),
    //                        algo::Position {
    //                            amount: order.amount,
    //                        },
    //                    );
    //                    lock.order_history.insert(symbol.clone(), order.clone());
    //                    lock.orders.remove(&symbol);
    //                }
    //            }
    //        }
    //    }
    //});
    //
    //let state4 = state.clone();
    //let status_task = tokio::task::spawn(async move {
    //    loop {
    //        tokio::time::sleep(Duration::from_secs(60)).await;
    //        let lock = state4.lock().await;
    //        info!("-- bank={}", lock.bank);
    //        info!("-- orders");
    //        for (symbol, order) in lock.orders.iter() {
    //            info!(
    //                "---- {:?} {} {} for {}",
    //                order.order_type, order.amount, symbol, order.price
    //            );
    //        }
    //        info!("-- order history");
    //        for (symbol, order) in lock.order_history.iter() {
    //            info!(
    //                "---- {:?} {} {} for {}",
    //                order.order_type, order.amount, symbol, order.price
    //            );
    //        }
    //    }
    //});
    //
    //let listen_task = tokio::task::spawn(async move {
    //    binance::kline_stream(
    //        vec![
    //            String::from("bnbusdt"),
    //            String::from("btcusdt"),
    //            String::from("xnousdt"),
    //        ],
    //        tx,
    //    )
    //    .await;
    //});
    //
    //tokio::select! {
    //    _ = listen_task => {}
    //    _ = algo_task => {}
    //    _ = orders_task => {}
    //    _ = status_task => {}
    //}
}
