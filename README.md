```shell
# run bot
cargo run -- start --server-address=127.0.0.1:5555 --replay-path=./data/events.jsonl --quote USDC --symbol BTCUSDC --symbol BNBUSDC
```

```shell
# watch bot
cargo run -- tui --server-address=127.0.0.1:5555 --quote USDC
```

```shell
# watch replay
cargo run -- tui --server-address=127.0.0.1:5554 --quote USDC
# start replay
cargo run -- replay --replay-path ./data/events.jsonl --quote USDC --server-address 127.0.0.1:5554 --symbol BTCUSDC --interval 10
```
