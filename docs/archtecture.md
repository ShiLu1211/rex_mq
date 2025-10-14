```
rex_mq/
├── Cargo.toml
├── README.md
├── docs/                           # 文档
│   ├── architecture.md            # 架构设计
│   ├── protocol.md                # 协议规范
│   └── quickstart.md              # 快速开始
│
├── src/
│   ├── lib.rs                     # 库入口
│   ├── main.rs                    # CLI 入口
│   │
│   ├── core/                      # 核心抽象层
│   │   ├── mod.rs
│   │   ├── client.rs              # RexClient trait
│   │   ├── server.rs              # RexServer trait
│   │   ├── sender.rs              # RexSender trait
│   │   ├── handler.rs             # RexClientHandler trait
│   │   ├── config.rs              # 配置定义
│   │   │   ├── client_config.rs
│   │   │   └── server_config.rs
│   │   └── error.rs               # 统一错误类型
│   │
│   ├── protocol/                  # 协议层
│   │   ├── mod.rs
│   │   ├── command.rs             # 命令定义
│   │   ├── retcode.rs             # 返回码
│   │   ├── data.rs                # 数据包结构
│   │   └── codec.rs               # 编解码器
│   │
│   ├── transport/                 # 传输层实现
│   │   ├── mod.rs
│   │   │
│   │   ├── tcp/                   # TCP 实现
│   │   │   ├── mod.rs
│   │   │   ├── client.rs
│   │   │   ├── server.rs
│   │   │   └── sender.rs
│   │   │
│   │   └── quic/                  # QUIC 实现
│   │       ├── mod.rs
│   │       ├── client.rs
│   │       ├── server.rs
│   │       ├── sender.rs
│   │       └── tls.rs             # TLS 配置
│   │
│   ├── system/                    # 系统管理
│   │   ├── mod.rs
│   │   ├── registry.rs            # RexSystem (客户端注册表)
│   │   └── client_info.rs         # RexClientInner (客户端信息)
│   │
│   ├── handler/                   # 消息处理器
│   │   ├── mod.rs
│   │   ├── dispatcher.rs          # 消息分发
│   │   ├── login.rs               # 登录处理
│   │   ├── title.rs               # 单播处理
│   │   ├── group.rs               # 组播处理
│   │   ├── cast.rs                # 广播处理
│   │   ├── check.rs               # 心跳检查
│   │   ├── reg_title.rs           # 注册 title
│   │   └── del_title.rs           # 删除 title
│   │
│   ├── aggregate/                 # 聚合服务器 (多协议支持)
│   │   ├── mod.rs
│   │   ├── server.rs              # AggregateServer
│   │   ├── builder.rs             # 构建器
│   │   └── config.rs              # 配置
│   │
│   ├── cluster/                   # 集群支持 (未来)
│   │   ├── mod.rs
│   │   ├── node.rs                # 节点管理
│   │   ├── discovery.rs           # 服务发现
│   │   └── replication.rs         # 数据复制
│   │
│   ├── storage/                   # 持久化 (未来)
│   │   ├── mod.rs
│   │   ├── wal.rs                 # Write-Ahead Log
│   │   ├── snapshot.rs            # 快照
│   │   └── engine.rs              # 存储引擎
│   │
│   ├── metrics/                   # 监控指标 (未来)
│   │   ├── mod.rs
│   │   ├── collector.rs           # 指标收集
│   │   └── exporter.rs            # Prometheus 导出
│   │
│   └── utils/                     # 工具函数
│       ├── mod.rs
│       ├── time.rs                # 时间相关
│       ├── id.rs                  # ID 生成
│       └── backoff.rs             # 退避算法
│
├── examples/                      # 示例代码
│   ├── tcp_simple.rs
│   ├── quic_simple.rs
│   ├── aggregate_server.rs        # 多协议服务器示例
│   └── benchmark.rs
│
├── tests/                         # 集成测试
│   ├── common/
│   │   └── mod.rs                 # 测试工具
│   ├── tcp_test.rs
│   ├── quic_test.rs
│   ├── aggregate_test.rs
│   └── integration_test.rs
│
└── benches/                       # 性能测试
    ├── throughput.rs
    └── latency.rs
```