use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::Colorize;
use futures::future;
use marketplace::binance::Binance;
use marketplace::*;
use order::{OrderSide, OrderStatus, OrderTrade};
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
use tokio::select;
use tokio::sync::RwLock;
use tokio_tungstenite::connect_async;
use tracing::{error, info, trace};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use trading_bot::marketplace::MarketPlaceStream;
use trading_bot::state::StateEvent;
use trading_bot::strategy::StrategyAction;
use tungstenite::Message;

use futures::StreamExt;
use trading_bot::tui::app::App;
use trading_bot::*;

#[derive(Parser, Debug)]
struct Args {
    #[arg(long)]
    replay_path: Option<PathBuf>,
    #[arg(long)]
    store_path: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    AccountInfo,
    Start {
        #[arg(long, value_delimiter = ',', default_value = "BTCUSDT")]
        symbol: Vec<String>,
        #[arg(long, default_value = "127.0.0.1:5555")]
        server_address: String,
        #[arg(long)]
        replay_path: Option<PathBuf>,
        #[arg(long)]
        real: bool,
    },
    Replay {
        #[arg(long, value_delimiter = ',', default_value = "BTCUSDT")]
        symbol: Vec<String>,
        #[arg(long, default_value = "127.0.0.1:5554")]
        server_address: String,
        #[arg(long)]
        no_server: bool,
        #[arg(long)]
        replay_path: PathBuf,
    },
    Tui {
        #[arg(long, default_value = "127.0.0.1:5555")]
        server_address: String,
    },
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

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

    match args.command {
        Some(Commands::AccountInfo) => {
            let _ = run_account_info().await;
        }
        Some(Commands::Start {
            symbol,
            replay_path,
            server_address,
            real,
        }) => {
            let tickers: Vec<Ticker> = symbol
                .iter()
                .flat_map(|symbol| Ticker::try_from(symbol))
                .collect();
            let _ = run_start(tickers, replay_path, server_address, real).await;
        }
        Some(Commands::Replay {
            symbol,
            replay_path,
            server_address,
            no_server,
        }) => {
            let tickers: Vec<Ticker> = symbol
                .iter()
                .flat_map(|symbol| Ticker::try_from(symbol))
                .collect();
            let _ = run_replay(
                tickers,
                replay_path,
                if no_server {
                    None
                } else {
                    Some(server_address)
                },
            )
            .await;
        }
        Some(Commands::Tui { server_address }) => {
            let _ = run_tui(server_address).await;
        }
        _ => {}
    }
}

async fn run_account_info() -> Result<()> {
    let marketplace = Binance::new();
    let account_overview = marketplace.get_account_overview(true).await;
    info!("{:?}", account_overview);

    Ok(())
}

async fn run_tui(server_address: String) -> Result<()> {
    let (tx, rx) = tokio::sync::mpsc::channel::<AppEvent>(100);
    let mut app = App::new(rx);

    let ws_client_task = tokio::task::spawn(async move {
        // (re)connect loop
        loop {
            let mut stream;
            // wait for server
            loop {
                let url = format!("ws://{}/ws", server_address);
                if let Ok(res) = connect_async(url).await {
                    stream = res.0;
                    break;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }

            let tx = tx.clone();

            // forward websocket messages to tui channel
            while let Some(Ok(msg)) = stream.next().await {
                match msg {
                    Message::Text(msg) => {
                        if let Ok(event) = serde_json::de::from_slice::<AppEvent>(msg.as_bytes()) {
                            let _ = tx.send(event).await;
                        }
                    }
                    Message::Close(frame) => {
                        break;
                    }
                    _ => {}
                };
            }

            // connection closed by server, wait before reconnecting
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    });

    let app_task = tokio::task::spawn(async move {
        let _ = app.run().await;
    });

    select! {
        _ = app_task => {},
        _ = ws_client_task => {}
    }

    ratatui::restore();

    Ok(())
}

async fn run_start(
    tickers: Vec<Ticker>,
    replay_path: Option<PathBuf>,
    server_address: String,
    real: bool,
) -> Result<()> {
    let state: Arc<RwLock<state::State>> = Arc::from(RwLock::from(state::State::new()));

    let (tx_app, _) = tokio::sync::broadcast::channel::<AppEvent>(1000);

    let mut marketplace = Binance::new();
    marketplace.init(&tickers).await?;

    {
        let state = state.clone();
        let mut state = state.write().await;
        if real {
            state.portfolio.assets = marketplace.get_account_assets().await?;
        } else {
            state.portfolio.assets = tickers
                .iter()
                .map(|ticker| {
                    (
                        ticker.base.clone(),
                        Asset {
                            symbol: ticker.base.clone(),
                            amount: dec!(0),
                            value: None,
                        },
                    )
                })
                .collect();
            state.portfolio.assets.insert(
                "USDT".to_string(),
                Asset {
                    symbol: "USDT".to_string(),
                    amount: dec!(5000),
                    value: None,
                },
            );
        }
    }

    tokio::task::spawn({
        let state = state.clone();
        let tickers = tickers.clone();
        let tx_app = tx_app.clone();
        let mut rx = tx_app.subscribe();
        let marketplace = marketplace.clone();

        async move {
            let mut strategy = ScalpingStrategy::new(state.clone(), marketplace, tickers);

            loop {
                let event = rx.recv().await;
                if let Ok(AppEvent::MarketPlace(event)) = event {
                    match strategy.on_marketplace_event(event).await {
                        Ok(action) => {
                            process_strategy_action(state.clone(), action, tx_app.clone()).await;
                        }
                        Err(_) => {}
                    }
                }
            }
        }
    });

    match replay_path {
        Some(replay_path) => tokio::task::spawn({
            let tx_app = tx_app.clone();
            let mut rx = tx_app.subscribe();

            async move {
                let mut file = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(replay_path)
                    .await
                    .expect("Failed to open file for storing events");

                loop {
                    // wait for a marketplace event
                    let event = rx.recv().await;
                    if let Ok(AppEvent::MarketPlace(event)) = event {
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
        }),
        None => tokio::task::spawn({
            let future = future::pending();
            async move {
                let () = future.await;
            }
        }),
    };

    tokio::task::spawn({
        let state = state.clone();
        let tx_app = tx_app.clone();
        async move {
            simulate_new_orders_processing(state, tx_app, false).await;
        }
    });

    tokio::task::spawn({
        let tx_app = tx_app.clone();
        let state = state.clone();
        let marketplace = marketplace.clone();

        async move {
            simulate_orders_processing(state, marketplace, tx_app, false).await;
        }
    });

    tokio::task::spawn({
        let state = state.clone();
        let tx_app = tx_app.clone();
        async move {
            update_portfolio_value(state, tx_app).await;
        }
    });

    tokio::task::spawn({
        let state = state.clone();
        async move { print_overview(state).await }
    });

    let marketplace_task = tokio::task::spawn({
        let mut marketplace = marketplace.clone();
        let tx_app = tx_app.clone();
        let tickers = tickers.clone();

        async move {
            marketplace.start(&tickers, tx_app).await;
        }
    });

    let server_task = tokio::task::spawn({
        let state = state.clone();
        let tx_app = tx_app.clone();
        async move { server::start(server_address, state, tx_app).await }
    });

    info!("{}", "STARTING BOT".green());

    tokio::select! {
        _ = marketplace_task => {}
        _ = server_task => {}
    }

    Ok(())
}

async fn run_replay(
    tickers: Vec<Ticker>,
    replay_path: PathBuf,
    server_address: Option<String>,
) -> Result<()> {
    let state: Arc<RwLock<state::State>> = Arc::from(RwLock::from(state::State::new()));

    let (tx_app, _) = tokio::sync::broadcast::channel::<AppEvent>(1000);

    let mut marketplace = Binance::new();
    marketplace.init(&tickers).await?;

    {
        let state = state.clone();
        let mut state = state.write().await;
        state.portfolio.assets = tickers
            .iter()
            .map(|ticker| {
                (
                    ticker.base.clone(),
                    Asset {
                        symbol: ticker.base.clone(),
                        amount: dec!(0),
                        value: None,
                    },
                )
            })
            .collect();
        state.portfolio.assets.insert(
            "USDT".to_string(),
            Asset {
                symbol: "USDT".to_string(),
                amount: dec!(5000),
                value: None,
            },
        );
    }

    tokio::task::spawn({
        let state = state.clone();
        let tickers = tickers.clone();
        let tx_app = tx_app.clone();
        let mut rx = tx_app.subscribe();
        let marketplace = marketplace.clone();

        async move {
            let mut strategy = ScalpingStrategy::new(state.clone(), marketplace, tickers);
            loop {
                // wait for a marketplace event
                let event = rx.recv().await;
                if let Ok(AppEvent::MarketPlace(event)) = event {
                    match strategy.on_marketplace_event(event).await {
                        Ok(action) => {
                            process_strategy_action(state.clone(), action, tx_app.clone()).await;
                        }
                        Err(_) => {}
                    }
                }
            }
        }
    });

    tokio::task::spawn({
        let state = state.clone();
        let tx_app = tx_app.clone();
        async move {
            simulate_new_orders_processing(state, tx_app, true).await;
        }
    });

    tokio::task::spawn({
        let tx_app = tx_app.clone();
        let state = state.clone();
        let marketplace = marketplace.clone();

        async move {
            simulate_orders_processing(state, marketplace, tx_app, true).await;
        }
    });

    tokio::task::spawn({
        let state = state.clone();
        let tx_app = tx_app.clone();
        async move {
            update_portfolio_value(state, tx_app).await;
        }
    });

    tokio::task::spawn({
        let state = state.clone();
        async move { print_overview(state).await }
    });

    let marketplace_task = tokio::task::spawn({
        let tx_app = tx_app.clone();

        async move {
            let file = File::open(replay_path)
                .await
                .expect("Failed to open replay file");
            let reader = BufReader::new(file);
            let mut lines = reader.lines();

            let mut i = 0;
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(event) = serde_json::de::from_str(&line) {
                    tx_app.send(AppEvent::MarketPlace(event)).unwrap();
                    i += 1;
                    if i % 1000 == 0 {
                        tokio::time::sleep(Duration::from_micros(10)).await;
                    }
                }
            }
        }
    });

    let server_task = match server_address {
        Some(server_address) => tokio::task::spawn({
            let state = state.clone();
            let tx_app = tx_app.clone();
            async move {
                let _ = server::start(server_address, state, tx_app).await;
            }
        }),
        None => tokio::task::spawn({
            let future = future::pending();
            async move {
                let () = future.await;
            }
        }),
    };

    info!("{}", "STARTING REPLAY".green());

    tokio::select! {
        _ = marketplace_task => { info!("marketplace_task ended"); }
        _ = server_task => { info!("server_task ended"); }
    }

    Ok(())
}

async fn simulate_new_orders_processing(
    state: Arc<RwLock<State>>,
    tx_app: tokio::sync::broadcast::Sender<AppEvent>,
    is_replay: bool,
) {
    let mut rx = tx_app.subscribe();
    loop {
        // get simulation time
        // this will "skip" one trade but it's actually more realistic
        let event = rx.recv().await;
        let time = if let Ok(AppEvent::MarketPlace(MarketPlaceEvent::Trade(event))) = event {
            event.trade_time
        } else {
            continue;
        };

        let mut state = state.write().await;
        let State { orders, .. } = &mut *state;

        let mut processed = false;

        for order in orders
            .iter_mut()
            .filter(|order| matches!(order.status, OrderStatus::Sent))
        {
            if order.creation_time + 2000_u64 < time {
                order.status = OrderStatus::Active;
                order.working_time = Some(time);
            }
            processed = true;
        }

        if processed {
            let _ = tx_app.send(AppEvent::State(StateEvent::Orders(orders.clone())));
        }

        processed = false;
        for order in orders
            .iter_mut()
            .filter(|order| order.status == OrderStatus::Draft)
        {
            // simulate sending request to markeplace
            if !is_replay {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            order.status = OrderStatus::Sent;
            processed = true;
        }

        if processed {
            let _ = tx_app.send(AppEvent::State(StateEvent::Orders(orders.clone())));
        }
    }
}

// listen to public trade events
// when a trade is executed, fill (partially or fully) a matching active order
async fn simulate_orders_processing<M>(
    state: Arc<RwLock<State>>,
    marketplace: M,
    tx_app: tokio::sync::broadcast::Sender<AppEvent>,
    is_replay: bool,
) where
    M: MarketPlace + MarketPlaceTrade + MarketPlaceSettings,
{
    let mut rx = tx_app.subscribe();
    loop {
        let event = rx.recv().await;
        if let Ok(AppEvent::MarketPlace(MarketPlaceEvent::Trade(event))) = event {
            let mut processed = false;
            let mut state = state.write().await;

            let State {
                portfolio, orders, ..
            } = &mut *state;

            // find active order
            for order in orders.iter_mut().filter(|order| {
                matches!(order.status, OrderStatus::Active) && order.ticker == event.ticker
            }) {
                // simulate if a trade will eat part of the order
                let trade_amount = match order.order_type {
                    order::OrderType::Market => Some(event.quantity * dec!(0.1)),
                    order::OrderType::Limit => {
                        match order.side {
                            // sell order : if the trade price >= order price, it will be eaten
                            OrderSide::Sell => {
                                if event.price >= order.price {
                                    Some(event.quantity * dec!(0.1))
                                } else {
                                    None
                                }
                            }
                            // buy order : if the trade price <= order price, it will be eaten
                            OrderSide::Buy => {
                                if event.price <= order.price {
                                    Some(event.quantity * dec!(0.1))
                                } else {
                                    None
                                }
                            }
                        }
                    }
                    _ => None,
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

                let to_fulfill = order.amount - order.filled_amount;
                let order_trade = if to_fulfill > trade_amount {
                    order.filled_amount += trade_amount;
                    info!(
                        "{} for order {} {}/{} : +{}",
                        match order.side {
                            OrderSide::Buy => "PARTIAL BUY".green(),
                            OrderSide::Sell => "PARTIAL SELL".green(),
                        },
                        Ticker::to_string(&event.ticker),
                        order.filled_amount,
                        order.amount,
                        trade_amount
                    );
                    OrderTrade {
                        trade_time: event.trade_time,
                        amount: trade_amount,
                        price: event.price,
                    }
                } else {
                    order.status = OrderStatus::Executed;
                    order.filled_amount = order.amount;
                    info!(
                        "{} for order {} {}/{} : +{}",
                        match order.side {
                            OrderSide::Buy => "FINAL BUY".green(),
                            OrderSide::Sell => "FINAL SELL".green(),
                        },
                        Ticker::to_string(&event.ticker),
                        order.filled_amount,
                        order.amount,
                        to_fulfill
                    );
                    OrderTrade {
                        trade_time: event.trade_time,
                        amount: to_fulfill,
                        price: event.price,
                    }
                };

                order.trades.push(order_trade.clone());

                let fees = marketplace.get_fees(order).await;

                let price = match order.order_type {
                    order::OrderType::Market => event.price,
                    _ => order.price,
                };

                match order.side {
                    OrderSide::Sell => {
                        // increase portfolio quote asset
                        let added_quote_amount = price * order_trade.amount * (dec!(1) - fees);
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
                            price,
                        );
                    }
                    OrderSide::Buy => {
                        // increase portfolio base asset
                        let added_base_amount = order_trade.amount * (dec!(1) - fees);
                        portfolio.update_asset_amount(&order.ticker.base, added_base_amount, price);

                        // decrease portfolio quote asset
                        // no fees, they apply to the bought asset
                        let removed_quote_amount = price * order_trade.amount;
                        portfolio.update_asset_amount(
                            &order.ticker.quote,
                            -removed_quote_amount,
                            dec!(1),
                        );
                    }
                }
                processed = true;
            }
            if !is_replay {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            if processed {
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
}

async fn update_portfolio_value(
    state: Arc<RwLock<State>>,
    tx_app: tokio::sync::broadcast::Sender<AppEvent>,
) {
    let mut rx = tx_app.subscribe();
    loop {
        let event = rx.recv().await;
        if let Ok(AppEvent::MarketPlace(MarketPlaceEvent::Trade(event))) = event {
            let mut state = state.write().await;
            if let Some(asset) = state.portfolio.assets.get_mut(&event.ticker.base) {
                asset.value = Some(asset.amount * event.price);
                state.portfolio.update_value();
            }
        }
    }
}

async fn print_overview(state: Arc<RwLock<State>>) {
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

async fn process_strategy_action(
    state: Arc<RwLock<State>>,
    action: StrategyAction,
    tx: tokio::sync::broadcast::Sender<AppEvent>,
) {
    match &action {
        StrategyAction::Order { order } => {
            info!("{:?}", order);
            let mut state = state.write().await;
            let _ = state.add_order(order.clone());
        }
        _ => {}
    }
    let _ = tx.send(AppEvent::Strategy(strategy::StrategyEvent::Action(action)));
}
