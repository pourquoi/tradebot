```shell
# run bot
RUST_LOG=info cargo run --bin bot -- --address=127.0.0.1:5555 --store-path=./data/events.jsonl
```

```shell
# run tui
RUST_LOG=info cargo run --bin tui -- --address=127.0.0.1:5555
```

```shell
# trace events
websocat ws://127.0.0.1:5555/ws
```
