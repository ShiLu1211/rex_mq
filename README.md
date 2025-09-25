# RexMq
## structure
```
.
├── Cargo.toml
├── CHANGELOG.md
├── examples
│   ├── port_refuse.rs
│   └── tcp.rs
├── README.md
├── src
│   ├── common // 基础配置
│   │   ├── client_config.rs
│   │   ├── client_handler.rs
│   │   ├── client.rs
│   │   ├── mod.rs
│   │   └── server_config.rs
│   ├── handler // handlers
│   │   └── mod.rs
│   ├── lib.rs
│   ├── main.rs
│   ├── net // 非协议相关trait
│   │   ├── client.rs
│   │   ├── mod.rs
│   │   ├── sender.rs
│   │   ├── server.rs
│   │   └── types.rs
│   ├── protocol // 数据格式
│   │   ├── command.rs
│   │   ├── data.rs
│   │   ├── error.rs
│   │   ├── mod.rs
│   │   └── retcode.rs
│   ├── quic 
│   │   ├── mod.rs
│   │   ├── quic_client.rs
│   │   ├── quic_sender.rs
│   │   └── quic_server.rs
│   ├── system // 管理client
│   │   ├── mod.rs
│   │   └── system.rs
│   ├── tcp
│   │   ├── mod.rs
│   │   ├── tcp_client.rs
│   │   ├── tcp_sender.rs
│   │   └── tcp_server.rs
│   └── utils // 工具
│       └── mod.rs
└── tests
    ├── base_test.rs
    ├── common.rs
    └── connect_test.rs
```

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