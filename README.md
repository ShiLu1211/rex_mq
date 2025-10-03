# RexMq

## example
``` bash
cargo run (-r) server -a [127.0.0.1:8881]

cargo run (-r) recv -a [127.0.0.1:8881] -t [one] (-b)

cargo run (-r) bench -a [127.0.0.1:8881] -y [title] -t [one] -i [100] (-b)
```

## tests
``` bash
cargo test --tests
cargo run --example port_refuse
```

## docs
```
tcp len 1024 tps 5w latency 70us(wsl) 35us(ubuntu)

```