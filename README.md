# RexMq

## example
``` bash
cargo run (-r) server -p [tcp|quic|websocket] -a [127.0.0.1:8881]

cargo run (-r) recv -p [tcp|quic|websocket] -a [127.0.0.1:8881] -t [one] (-b)

cargo run (-r) bench -p [tcp|quic|websocket] -a [127.0.0.1:8881] -y [title] -t [one] -i [100] (-b)
```

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
```
tcp len 1024 tps 5w latency 50-60us(wsl) 35us(ubuntu)
```