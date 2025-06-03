use clap::{Parser, Subcommand};
use colored::Colorize;
use futures::future;
use marketplace::binance::Binance;
use marketplace::{MarketPlace, MarketPlaceEvent};
use order::{OrderStatus, OrderTrade, OrderType};
use portfolio::Asset;
use rust_decimal_macros::dec;
use state::State;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use strategy::buy_and_hold::BuyAndHoldStrategy;
use strategy::StrategyEvent;
use strategy::{scalping::ScalpingStrategy, Strategy};
use ticker::Ticker;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::select;
use tokio::sync::RwLock;
use tracing::{error, info, trace};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use trading_bot::state::StateEvent;

use trading_bot::*;

#[derive(Parser, Debug)]
struct Args {
    #[arg(long)]
    replay_path: Option<PathBuf>,
    #[arg(long)]
    store_path: Option<PathBuf>,
    #[arg(long, default_value = "127.0.0.1:5555")]
    address: String,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    AccountInfo,
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    // fmt tracing subscriber -> log to stdout
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            format!(
                "{}=debug,tower_http=debug,reqwest=debug",
                env!("CARGO_CRATE_NAME")
            )
            .into()
        }))
        .with(fmt::layer())
        .init();

    let args = Args::parse();
    let is_replay = args.replay_path.is_some();

    let mut marketplace = Binance::new();
    marketplace
        .ping()
        .await
        .expect("Failed to ping marketplace");

    match &args.command {
        Some(Commands::AccountInfo) => {
            let account_overview = marketplace.get_account_overview().await;
            info!("{:?}", account_overview);
            return;
        }
        _ => {}
    }

    let tickers = vec![
        Ticker::new("BTC", "USDT"),
        Ticker::new("ETH", "USDT"),
        Ticker::new("BNB", "USDT"),
        Ticker::new("XNO", "USDT"),
    ];

    match marketplace.init(tickers.clone()).await {
        Ok(()) => {}
        Err(_err) => {
            return;
        }
    }

    let state: Arc<RwLock<state::State>> = Arc::from(RwLock::from(state::State::new()));

    {
        let state = state.clone();
        let mut state = state.write().await;
        state.portfolio.assets.insert(
            String::from("USDT"),
            Asset {
                amount: dec!(5000),
                symbol: String::from("USDT").to_uppercase(),
                value: None,
            },
        );
        for ticker in tickers.iter() {
            state.portfolio.assets.insert(
                ticker.base.clone(),
                Asset {
                    symbol: ticker.base.clone(),
                    amount: dec!(0),
                    value: None,
                },
            );
        }
    }

    let (tx_app, mut rx_app) = tokio::sync::broadcast::channel::<AppEvent>(100);
    let (tx_marketplace, mut rx) =
        tokio::sync::broadcast::channel::<marketplace::MarketPlaceEvent>(10000);
    let (tx_strategy, mut rx_strategy) = tokio::sync::broadcast::channel::<StrategyEvent>(100);

    // app event dispatcher
    let event_dispatcher = tokio::task::spawn({
        let tx_app = tx_app.clone();

        let tx_strategy = tx_strategy.clone();
        let mut rx_strategy = tx_strategy.subscribe();
        let tx_marketplace = tx_marketplace.clone();
        let mut rx_marketplace = tx_marketplace.subscribe();

        // listen on rx_strategy and rx_marketplace and dispatch first message to tx_app
        async move {
            loop {
                select! {
                    result = rx_marketplace.recv() => {
                        match result {
                            Ok(event) => { let _ = tx_app.send(AppEvent::MarketPlace(event)); }
                            _ => {}
                        };
                    },
                    result = rx_strategy.recv() => {
                        match result {
                            Ok(event) => { let _ = tx_app.send(AppEvent::Strategy(event)); }
                            _ => {}
                        };
                    }
                }
            }
        }
    });

    // websocket/http server
    let server_task = tokio::task::spawn({
        let state = state.clone();
        let tx_app = tx_app.clone();
        async move { server::start(args.address.clone(), state, tx_app).await }
    });

    // spawn strategy task
    let strategy_task = tokio::task::spawn({
        let state = state.clone();
        let tickers = tickers.clone();
        let tx_marketplace = tx_marketplace.clone();
        let mut rx = tx_marketplace.subscribe();
        let tx_strategy = tx_strategy.clone();
        let marketplace = marketplace.clone();

        async move {
            //let mut strategy = BuyAndHoldStrategy::new(state.clone(), marketplace, tickers);
            let mut strategy =
                ScalpingStrategy::new(state.clone(), marketplace, tickers, tx_strategy);
            loop {
                // wait for a marketplace event
                let event = rx.recv().await;
                if let Ok(event) = event {
                    strategy.on_marketplace_event(event, state.clone()).await;
                }
            }
        }
    });

    // spawn marketplace task
    let marketplace_task = tokio::task::spawn({
        let tickers = tickers.clone();
        let tx_marketplace = tx_marketplace.clone();
        let mut marketplace = marketplace.clone();

        async move {
            if let Some(path) = args.replay_path {
                // todo
                // load events form file and send them on tx
                let file = File::open(path).await.expect("Failed to open replay file");
                let reader = BufReader::new(file);
                let mut lines = reader.lines();

                let mut i = 0;
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Ok(event) = serde_json::de::from_str(&line) {
                        tx_marketplace.send(event).unwrap();
                        i += 1;
                        if i % 1000 == 0 {
                            tokio::time::sleep(Duration::from_micros(10)).await;
                        }
                    }
                }
            } else {
                marketplace.start(tickers, tx_marketplace).await;
            }
        }
    });

    // save marketplace events for backtesting
    let store_events_task = match args.store_path {
        Some(path) => {
            tokio::task::spawn({
                let tx_marketplace = tx_marketplace.clone();
                let mut rx = tx_marketplace.subscribe();

                async move {
                    let mut file = OpenOptions::new()
                        .create(true)
                        .append(true)
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
        let tx_marketplace = tx_marketplace.clone();
        let tx_app = tx_app.clone();
        let mut rx = tx_marketplace.subscribe();
        async move {
            loop {
                // get simulation time
                // this will "skip" one trade but it's actually more realistic
                let event = rx.recv().await;
                let mut time: Option<u64> = None;
                if let Ok(event) = event {
                    match event {
                        MarketPlaceEvent::Trade(event) => {
                            time = Some(event.trade_time);
                        }
                        _ => {}
                    }
                }

                if time.is_none() {
                    continue;
                }
                let time = time.unwrap();

                let mut state = state.write().await;
                let State { orders, .. } = &mut *state;

                let mut processed = false;

                for order in orders
                    .iter_mut()
                    .filter(|order| matches!(order.order_status, OrderStatus::Sent { .. }))
                {
                    if let OrderStatus::Sent { ts } = order.order_status {
                        if ts + 2000_u64 < time {
                            order.order_status = OrderStatus::Active { ts: time + 500_u64 }
                        }
                    }
                    processed = true;
                }

                if processed {
                    let _ = tx_app.send(AppEvent::State(StateEvent::Orders(orders.clone())));
                }

                processed = false;
                for order in orders
                    .iter_mut()
                    .filter(|order| order.order_status == OrderStatus::Draft)
                {
                    // simulate sending request to markeplace
                    if !is_replay {
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                    order.order_status = OrderStatus::Sent { ts: time };
                    processed = true;
                }

                if processed {
                    let _ = tx_app.send(AppEvent::State(StateEvent::Orders(orders.clone())));
                }
            }
        }
    });

    // spawn orders results task
    let orders_result_task = tokio::task::spawn({
        let tx_app = tx_app.clone();
        let state = state.clone();
        let tx_marketplace = tx_marketplace.clone();
        let mut rx = tx_marketplace.subscribe();
        let marketplace = marketplace.clone();

        async move {
            loop {
                let event = rx.recv().await;
                let mut processed = false;
                if let Ok(event) = event {
                    match event {
                        MarketPlaceEvent::Trade(event) => {
                            let mut state = state.write().await;

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
                                    trace!("No trade for order");
                                    continue;
                                }
                                let trade_amount = trade_amount.unwrap();

                                // simulate a marketplace request
                                if !is_replay {
                                    tokio::time::sleep(Duration::from_secs(1)).await;
                                }

                                let to_fullfill = order.amount - order.fullfilled;
                                let order_trade = if to_fullfill > trade_amount {
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
                                    OrderTrade {
                                        trade_time: event.trade_time,
                                        amount: trade_amount,
                                        price: event.price,
                                    }
                                } else {
                                    order.order_status = OrderStatus::Executed {
                                        ts: event.trade_time,
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
                                    OrderTrade {
                                        trade_time: event.trade_time,
                                        amount: to_fullfill,
                                        price: event.price,
                                    }
                                };

                                order.trades.push(order_trade.clone());

                                let fees = marketplace.get_fees(order).await;

                                match order.order_type {
                                    OrderType::Sell => {
                                        // increase portfolio quote asset
                                        let added_quote_amount = order_trade.price
                                            * order_trade.amount
                                            * (dec!(1) - fees);
                                        portfolio.update_asset_amount(
                                            &order.ticker.quote,
                                            added_quote_amount,
                                            dec!(1),
                                        );

                                        // decrease portfolio base asset
                                        //
                                        // todo : check if fees apply
                                        //let removed_base_amount = order_trade.amount;
                                        //    order_trade.amount * (dec!(1) - fees);
                                        let removed_base_amount = order_trade.amount;
                                        portfolio.update_asset_amount(
                                            &order.ticker.base,
                                            -removed_base_amount,
                                            event.price,
                                        );
                                    }
                                    OrderType::Buy => {
                                        // increase portfolio base asset
                                        let added_base_amount =
                                            order_trade.amount * (dec!(1) - fees);
                                        portfolio.update_asset_amount(
                                            &order.ticker.base,
                                            added_base_amount,
                                            event.price,
                                        );

                                        // decrease portfolio quote asset
                                        // no fees, they apply to the bought asset
                                        let removed_quote_amount =
                                            order_trade.price * order_trade.amount;
                                        portfolio.update_asset_amount(
                                            &order.ticker.quote,
                                            -removed_quote_amount,
                                            dec!(1),
                                        );
                                    }
                                }
                                processed = true;
                            }
                        }
                        _ => {}
                    }
                }
                if !is_replay {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                if processed {
                    let state = state.read().await;
                    info!(
                        "\n## Portfolio: \n{}\n## {} {}",
                        state.portfolio,
                        "Scalped".yellow(),
                        state.get_total_scalped("USDT".to_string())
                    );
                    let _ = tx_app.send(AppEvent::State(StateEvent::Portfolio(
                        state.portfolio.clone(),
                    )));
                    let _ = tx_app.send(AppEvent::State(StateEvent::Orders(state.orders.clone())));
                }
            }
        }
    });

    let update_portfolio_total_task = tokio::task::spawn({
        let state = state.clone();
        let tx_marketplace = tx_marketplace.clone();
        let mut rx = tx_marketplace.subscribe();
        async move {
            loop {
                let event = rx.recv().await;
                if let Ok(event) = event {
                    match event {
                        marketplace::MarketPlaceEvent::Trade(event) => {
                            let mut state = state.write().await;
                            if let Some(asset) = state.portfolio.assets.get_mut(&event.ticker.base)
                            {
                                asset.value = Some(asset.amount * event.price);
                                state.portfolio.update_value();
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
                    let state = state.read().await;
                    info!(
                        "\n## Portfolio: {}\n## {} {}",
                        state.portfolio,
                        "Scalped".yellow(),
                        state.get_total_scalped("USDT".to_string())
                    );
                }
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
        }
    });

    info!("{}", "STARTING BOT".green());

    tokio::select! {
        result = server_task => { info!("server task ended : {:?}", result); }
        _ = marketplace_task => { info!("maretplace task ended"); }
        _ = strategy_task => { info!("strategy task ended"); }
        _ = pending_orders_task => { info!("pending orders task ended"); }
        _ = orders_result_task => { info!("orders result task ended"); }
        _ = store_events_task => { info!("store events task ended"); }
        _ = update_portfolio_total_task => { info!("update protfolio task ended"); }
        _ = overview_task => { info!("overview task ended"); }
    }

    {
        let state = state.read().await;
        info!("{}", state.portfolio);
        info!(
            "\n## Portfolio: {}\n## {} {}",
            state.portfolio,
            "Scalped".yellow(),
            state.get_total_scalped("USDT".to_string())
        );
    }
}
