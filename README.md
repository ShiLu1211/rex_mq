# RexMq

## example

``` bash
# start server with defaults (TCP:8881, QUIC:8882, WebSocket:8883)
cargo run -- server

# start server from a config file
cargo run -- server --config ./rex.toml

# print the default config (all defaults applied)
cargo run -- --print-default-config

# print effective config after merging TOML + env + CLI
cargo run -- --config ./rex.toml --print-effective-config

cargo run (-r) recv -p [tcp|quic|websocket] -a [127.0.0.1:8881] -t [one] (-b)

cargo run (-r) bench -p [tcp|quic|websocket] -a [127.0.0.1:8881] -y [title] -t [one] -i [100] (-b)
```

## configuration

Server configuration uses a 4-layer cascade: **defaults → rex.toml → env → CLI**.

``` bash
# env-var override (12-factor)
REX__SERVER__CHECK_INTERVAL=5 REX__OBSERVABILITY__TRACING_FORMAT=json \
  cargo run -- server
```

An annotated `rex.toml`:

``` toml
[server]
server_id       = "rex-prod"
check_interval  = 30
client_timeout  = 120

[[endpoints]]
protocol = "tcp"
address  = "0.0.0.0:8881"

[[endpoints]]
protocol = "quic"
address  = "0.0.0.0:8882"

[persistence]
enabled = true
path    = "/var/lib/rex/rex_sled"

[persistence.offline]
ttl_secs = 604800

[observability]
admin_addr  = "127.0.0.1:9090"
tracing_format = "json"
```

Unknown keys in TOML cause the server to exit with code 78 (`EX_CONFIG`) at boot — typos are surfaced, not silently ignored.

## tests
``` bash
cargo test --tests
cargo run --example port_refuse
```

## java
``` bash
cargo build -r -p rex4j
cp target/release/librex4j.so bindings/rex4j/src/main/resources

# bindings/rex4j
mvn clean package

java -jar target/rex-rex4j-0.1.0.jar -y rcv -h 127.0.0.1 -p 8881 -t one

java -jar target/rex-rex4j-0.1.0.jar -y snd -h 127.0.0.1 -p 8881 -t one -c Title -s 1024 -i 100 -T 60
```

## python
``` bash
cargo build -r
cp target/release/librex4p.so bindings/rex4p/examples/rex4p.so

cd bindings/rex4p/examples

python3 ./rex_engine.py -H 127.0.0.1 -p 8881 -t one -y rcv

python3 ./rex_engine.py -H 127.0.0.1 -p 8881 -t one -y snd -s 1024 -i 50 -T 60
```

## docs

| Protocol | Client | Length | TPS | Latency |
|:----:|:----:|:----:|:----:|:----:|
| TCP | Rust | 1024 | 50000 | 30-us |
| | Java | | | 30us |
| | Python | | | 150-us |
| | Python | | 20000 | 60us |
