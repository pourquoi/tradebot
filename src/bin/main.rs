use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::Colorize;
use futures::future;
use futures_util::{SinkExt, StreamExt};
use marketplace::binance::Binance;
use marketplace::*;
use rust_decimal_macros::dec;
use state::State;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use strategy::{scalping::ScalpingStrategy, Strategy};
use ticker::Ticker;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::select;
use tokio::sync::RwLock;
use tokio_tungstenite::connect_async;
use tracing::{debug, error, info};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use trading_bot::marketplace::replay::ReplayMarketplace;
use trading_bot::marketplace::simulation::{SimulationMarketplace, SimulationSource};
use trading_bot::marketplace::MarketplaceDataStream;
use trading_bot::order::OrderStatus;
use trading_bot::strategy::{StrategyAction, StrategyEvent};
use tungstenite::Message;

use trading_bot::strategy::scalping::ScalpingParams;
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
        #[arg(long, value_delimiter = ',', default_value = "BTCUSDC")]
        symbol: Vec<String>,
        #[arg(long, default_value = "127.0.0.1:5555")]
        server_address: String,
        #[arg(long)]
        replay_path: Option<PathBuf>,
        #[arg(long)]
        real: bool,
        #[arg(long, default_value = "USDC")]
        quote: String,
    },
    Replay {
        #[arg(long, value_delimiter = ',', default_value = "BTCUSDC")]
        symbol: Vec<String>,
        #[arg(long, default_value = "127.0.0.1:5554")]
        server_address: String,
        #[arg(long)]
        no_server: bool,
        #[arg(long)]
        replay_path: PathBuf,
        #[arg(long, default_value = "USDC")]
        quote: String,
        #[arg(long, default_value = "1")]
        interval: u64,
    },
    Tui {
        #[arg(long, value_delimiter = ',', default_value = "BTCUSDC")]
        symbol: Vec<String>,
        #[arg(long, default_value = "127.0.0.1:5555")]
        server_address: String,
        #[arg(long, default_value = "USDC")]
        quote: String,
    },
    Test,
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
            quote,
            symbol,
            replay_path,
            server_address,
            real,
        }) => {
            let tickers: Vec<Ticker> = symbol
                .iter()
                .flat_map(Ticker::try_from)
                .collect();
            let _ = run_start(quote, tickers, replay_path, server_address, real).await;
        }
        Some(Commands::Replay {
            interval,
            quote,
            symbol,
            replay_path,
            server_address,
            no_server,
        }) => {
            let tickers: Vec<Ticker> = symbol
                .iter()
                .flat_map(Ticker::try_from)
                .collect();
            let _ = run_replay(
                interval,
                quote,
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
        Some(Commands::Tui {
            symbol,
            quote,
            server_address,
        }) => {
            let tickers: Vec<Ticker> = symbol
                .iter()
                .flat_map(Ticker::try_from)
                .collect();
            let _ = run_tui(quote, server_address, tickers).await;
        }
        Some(Commands::Test) => {
            run_test().await;
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

async fn run_tui(quote: String, server_address: String, tickers: Vec<Ticker>) -> Result<()> {
    let (tx, rx) = tokio::sync::mpsc::channel::<AppEvent>(100);
    let (tx_cmd, mut rx_cmd) = tokio::sync::mpsc::channel::<AppCommandEvent>(100);
    let mut app = App::new(rx, tx_cmd, quote, tickers);

    let ws_client_task = tokio::task::spawn(async move {
        // (re)connect loop
        loop {
            let stream;
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

            let (mut write, mut read) = stream.split();

            loop {
                select! {
                    msg = read.next() => {
                        match msg {
                            Some(Ok(Message::Text(msg))) => {
                                if let Ok(event) = serde_json::de::from_slice::<AppEvent>(msg.as_bytes()) {
                                    let _ = tx.send(event).await;
                                }
                            },
                        Some(Ok(Message::Close(_))) | None => {
                                    break;
                                }
                            _ => {}
                        }
                    }
                    Some(cmd) = rx_cmd.recv() => {
                        if let Ok(cmd) = serde_json::ser::to_string(&cmd) {
                            let _ = write.send(Message::Text(cmd.into())).await;
                        }
                    }
                }
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

async fn run_test() {
    let mut binance = Binance::new();

    let (tx_app, _) = tokio::sync::broadcast::channel::<AppEvent>(1000);
    let res = binance.start_account_stream(tx_app).await;

    println!("{:?}", res);
}

async fn run_start(
    quote: String,
    tickers: Vec<Ticker>,
    replay_path: Option<PathBuf>,
    server_address: String,
    real: bool,
) -> Result<()> {
    let state: Arc<RwLock<state::State>> = Arc::from(RwLock::from(state::State::new()));

    let (tx_app, _) = tokio::sync::broadcast::channel::<AppEvent>(1000);
    let (tx_cmd, _) = tokio::sync::mpsc::channel::<AppCommandEvent>(16);

    let mut marketplace = Binance::new();
    marketplace.init(&tickers).await?;

    let mut simulation =
        SimulationMarketplace::new(simulation::SimulationSource::Book, marketplace.clone());

    if !real {
        simulation
            .update_asset_amount(&quote, dec!(1000), Some(dec!(1)))
            .await;
        tokio::spawn({
            let tx_app = tx_app.clone();
            let mut simulation = simulation.clone();
            async move {
                info!("{}", "Starting simulation matching".green());
                let _ = simulation.start_matching(tx_app).await;
                info!("{}", "Ended simulation matching".red());
            }
        });
    }

    {
        let mut state = state.write().await;
        if real {
            state.portfolio.assets = marketplace.get_account_assets().await?;
            state.orders = marketplace.get_orders(&tickers).await?;
        } else {
            state.portfolio.assets = simulation.get_account_assets().await?;
        }
    }

    if real {
        tokio::spawn({
            let tx_app = tx_app.clone();
            let mut marketplace = marketplace.clone();
            async move {
                info!("{}", "Starting account stream".green());
                let _ = marketplace.start_account_stream(tx_app).await;
                info!("{}", "Ended account stream".red());
            }
        });
    } else {
        tokio::spawn({
            let tx_app = tx_app.clone();
            let mut simulation = simulation.clone();
            async move {
                info!("{}", "Starting simulation account stream".green());
                let _ = simulation.start_account_stream(tx_app).await;
                info!("{}", "Ended simulation account stream".red());
            }
        });
    }

    for ticker in tickers.iter() {
        let mut strategy = ScalpingStrategy::new(
            state.clone(),
            marketplace.clone(),
            ticker.clone(),
            ScalpingParams {
                target_profit: dec!(1),
                quote_amount: dec!(100),
                buy_cooldown: Duration::from_secs(60 * 15),
                multiple_orders: true,
            },
        );
        if strategy.init(None).await.is_err() {
            panic!("Failed strategy initialization for {ticker}");
        }

        tokio::task::spawn({
            let tx_app = tx_app.clone();
            async move {
                info!("{}", "Starting strategy".green());
                let _ = strategy.start(tx_app).await;
                info!("{}", "Ended strategy".red());
            }
        });
    }

    if let Some(replay_path) = replay_path {
        tokio::task::spawn({
            let tx_app = tx_app.clone();
            async move {
                log_marketplace_events(replay_path, tx_app).await;
            }
        });
    }

    tokio::task::spawn({
        let state = state.clone();
        let tx_app = tx_app.clone();
        let marketplace = marketplace.clone();
        let simulation = simulation.clone();
        async move {
            if real {
                process_app_event(state, marketplace, tx_app).await;
            } else {
                process_app_event(state, simulation, tx_app).await;
            }
        }
    });

    let marketplace_task = tokio::task::spawn({
        let mut marketplace = marketplace.clone();
        let tx_app = tx_app.clone();
        let tickers = tickers.clone();

        async move {
            info!("{}", "Starting data stream".green());
            let _ = marketplace.start_data_stream(&tickers, tx_app).await;
            info!("{}", "Ended data stream".red());
        }
    });

    let server_task = tokio::task::spawn({
        let state = state.clone();
        let tx_app = tx_app.clone();
        let tx_cmd = tx_cmd.clone();
        async move { server::start(server_address, state, tx_app, tx_cmd).await }
    });

    info!("{}", "STARTING BOT".green());

    tokio::select! {
        _ = marketplace_task => {}
        _ = server_task => {}
    }

    Ok(())
}

async fn run_replay(
    interval: u64,
    quote: String,
    tickers: Vec<Ticker>,
    replay_path: PathBuf,
    server_address: Option<String>,
) -> Result<()> {
    let state: Arc<RwLock<state::State>> = Arc::from(RwLock::from(state::State::new()));

    let (tx_app, _) = tokio::sync::broadcast::channel::<AppEvent>(10000);
    let (tx_cmd, mut rx_cmd) = tokio::sync::mpsc::channel::<AppCommandEvent>(16);

    let mut marketplace = Binance::new();
    marketplace.init(&tickers).await?;

    let replay = ReplayMarketplace::new(replay_path, marketplace.clone());

    let mut simulation = SimulationMarketplace::new(SimulationSource::Book, marketplace.clone());
    simulation
        .update_asset_amount(&quote, dec!(1000), Some(dec!(1)))
        .await;

    {
        let mut state = state.write().await;
        state.portfolio.assets = simulation.get_account_assets().await?;
    }

    let start_time = replay.get_start_time().await;
    if start_time.is_none() {
        panic!("Could not find start time from replay file");
    }

    for ticker in tickers.iter() {
        let mut strategy = ScalpingStrategy::new(
            state.clone(),
            marketplace.clone(),
            ticker.clone(),
            ScalpingParams {
                target_profit: dec!(1.5),
                quote_amount: dec!(100),
                buy_cooldown: Duration::from_secs(60),
                multiple_orders: true,
            },
        );
        if strategy.init(start_time).await.is_err() {
            panic!("Could not init strategy");
        }

        tokio::task::spawn({
            let tx_app = tx_app.clone();
            async move {
                info!("{}", "Starting strategy".green());
                let _ = strategy.start(tx_app).await;
                info!("{}", "Ended strategy".red());
            }
        });
    }

    tokio::spawn({
        let tx_app = tx_app.clone();
        let mut simulation = simulation.clone();
        async move {
            let _ = simulation.start_account_stream(tx_app).await;
        }
    });

    tokio::task::spawn({
        let tx_app = tx_app.clone();
        let state = state.clone();
        let marketplace = simulation.clone();

        async move {
            process_app_event(state, marketplace, tx_app).await;
        }
    });

    tokio::task::spawn({
        let mut replay = replay.clone();
        async move {
            loop {
                if let Some(cmd) = rx_cmd.recv().await {
                    info!("Received command {:?}", cmd);
                    match cmd {
                        AppCommandEvent::Pause => {
                            replay.toggle_pause().await;
                        }
                    }
                }
            }
        }
    });

    tokio::task::spawn({
        let mut simulation = simulation.clone();
        let tx_app = tx_app.clone();

        async move {
            info!("{}", "Starting simulation matching".green());
            let _ = simulation.start_matching(tx_app).await;
            info!("{}", "Ended simulation matching".red());
        }
    });
    let marketplace_task = tokio::task::spawn({
        let mut replay = replay.clone();
        let tx_app = tx_app.clone();

        async move {
            info!("{}", "Starting replay data stream".green());
            let _ = replay.start_data_stream(&tickers, tx_app).await;
            info!("{}", "Ended replay data stream".red());
        }
    });

    let server_task = match server_address {
        Some(server_address) => tokio::task::spawn({
            let state = state.clone();
            let tx_app = tx_app.clone();
            let tx_cmd = tx_cmd.clone();
            async move {
                let _ = server::start(server_address, state, tx_app, tx_cmd).await;
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

async fn process_app_event<T: MarketplaceTradeApi>(
    state: Arc<RwLock<State>>,
    mut marketplace: T,
    tx_app: tokio::sync::broadcast::Sender<AppEvent>,
) {
    let mut rx_app = tx_app.subscribe();
    loop {
        if let Ok(event) = rx_app.recv().await {
            if let AppEvent::MarketPlace(MarketplaceEvent::Book(book)) = &event {
                if let Some(price) = book.buy_price() {
                    let mut state = state.write().await;
                    state.portfolio.update_asset_value(&book.ticker.base, price);
                    let _ = tx_app.send(AppEvent::State(state::StateEvent::Portfolio(
                        state.portfolio.clone(),
                    )));
                }
            }

            match event {
                AppEvent::Strategy(StrategyEvent::Action(StrategyAction::None)) => {
                    debug!("{}", "No strategy".purple());
                }
                AppEvent::Strategy(StrategyEvent::Action(StrategyAction::Ignore {
                    ticker,
                    reason,
                    details,
                })) => {
                    debug!(
                        "{} {} {} {:?}",
                        "Ignore".purple(),
                        ticker,
                        reason.as_str().yellow(),
                        details
                    )
                }
                AppEvent::Strategy(StrategyEvent::Action(StrategyAction::PlaceOrder { order })) => {
                    info!("{} {:?}", "Add order".blue(), order);
                    let added_order = {
                        let mut state = state.write().await;
                        state.add_order(order.clone())
                    };
                    match added_order {
                        Ok(order) => match marketplace.place_order(&order).await {
                            Ok(market_order) => {
                                let mut state = state.write().await;
                                if let Some(order) = state.find_by_id(&order.id) {
                                    *order = market_order;
                                }
                            }
                            Err(err) => {
                                error!("Failed posting order : {err}");
                                let mut state = state.write().await;
                                if let Some(order) = state.find_by_id(&order.id) {
                                    order.status = OrderStatus::Rejected;
                                }
                            }
                        },
                        Err(err) => {
                            error!("Failed creating order : {err}");
                        }
                    }
                }
                AppEvent::MarketPlace(MarketplaceEvent::PortfolioUpdate(update)) => {
                    info!("{} : {:?}", "Portfolio update".blue(), update);
                    let mut state = state.write().await;
                    for asset in update.assets {
                        state.portfolio.update_asset(asset);
                    }
                    let _ = tx_app.send(AppEvent::State(state::StateEvent::Portfolio(
                        state.portfolio.clone(),
                    )));
                }
                AppEvent::MarketPlace(MarketplaceEvent::OrderUpdate(update)) => {
                    info!("{} : {:?}", "Order update".blue(), update);
                    let orders = {
                        let mut state = state.write().await;
                        state.update_order(update);
                        state.orders.clone()
                    };
                    let _ = tx_app.send(AppEvent::State(state::StateEvent::Orders(orders)));
                }
                _ => {}
            }
        }
    }
}

async fn log_marketplace_events(
    replay_path: PathBuf,
    tx_app: tokio::sync::broadcast::Sender<AppEvent>,
) {
    let mut rx = tx_app.subscribe();

    let mut path = replay_path.clone();
    path.push("events.jsonl");

    tokio::fs::create_dir_all(path.parent().unwrap())
        .await
        .expect("Failed to create log directories");

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
        .expect("Failed to open log file");

    loop {
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
                    error!("Failed to save event : {}", err);
                }
            }
        }
    }
}
