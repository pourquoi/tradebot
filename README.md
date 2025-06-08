```shell
# run bot
RUST_LOG=info cargo run -- start --server-address=127.0.0.1:5555 --replay-path=./data/events.jsonl --symbol BTCUSDT --symbol BNBUSDT
```

```shell
# run tui
RUST_LOG=info cargo run -- tui --server-address=127.0.0.1:5555
```
