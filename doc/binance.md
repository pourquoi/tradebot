futures market streams
---
[kline stream](https://developers.binance.com/docs/derivatives/usds-margined-futures/websocket-market-streams/Kline-Candlestick-Streams)
[aggregate trade stream](https://developers.binance.com/docs/derivatives/usds-margined-futures/websocket-market-streams/Aggregate-Trade-Streams)
[mark price stream](https://developers.binance.com/docs/derivatives/usds-margined-futures/websocket-market-streams/Mark-Price-Stream)

spot market streams
---
[trade](https://developers.binance.com/docs/binance-spot-api-docs/web-socket-streams#trade-streams)

trading api
---
[new order](https://developers.binance.com/docs/binance-spot-api-docs/rest-api/trading-endpoints)

keys
---
https://github.com/binance/binance-spot-api-docs/blob/master/testnet/web-socket-api.md#signed-request-example-ed25519
openssl genpkey -algorithm ed25519 -out priv.pem
openssl pkey -in priv.pem -pubout -out pub.pem
