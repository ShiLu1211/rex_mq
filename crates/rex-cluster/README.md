# RexMQ Cluster Module

分布式集群模块，支持 Leader-Follower 架构和 Gossip 协议节点发现。

## 架构概览

```
┌─────────────────────────────────────────────────────────────┐
│                     ClusterManager                          │
│  ┌─────────────┐  ┌─────────────┐  ┌──────────────────┐   │
│  │  Gossip     │  │  Failover   │  │  StateSyncer     │   │
│  │  Protocol   │  │  Manager    │  │  (Leader→Follow)│   │
│  └─────────────┘  └─────────────┘  └──────────────────┘   │
│         │                 │                  │               │
│  ┌──────┴─────────────────┴──────────────────┴──────────┐  │
│  │              ClusterTransport (TCP)                   │  │
│  └───────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                      GlobalRouteTable                        │
│  ┌─────────────────┐        ┌────────────────────────┐    │
│  │  HashRing       │        │  Client→Node Mapping    │    │
│  │  (Consistent   │        │  (Title Routing)        │    │
│  │   Hashing)     │        │                         │    │
│  └─────────────────┘        └────────────────────────┘    │
└─────────────────────────────────────────────────────────────┘
```

## 核心组件

| 模块 | 功能 |
|------|------|
| [types.rs](src/types.rs) | 基础类型定义 (NodeId, NodeInfo, ClusterMessage 等) |
| [transport.rs](src/transport.rs) | TCP 传输层，节点间通信 |
| [node.rs](src/node.rs) | 节点管理，Leader 选举/心跳 |
| [hash_ring.rs](src/hash_ring.rs) | 一致性哈希环 (Ketama 算法) |
| [route_table.rs](src/route_table.rs) | 全局路由表，客户端/标题路由 |
| [forward.rs](src/forward.rs) | 跨节点消息转发 |
| [gossip.rs](src/gossip.rs) | 节点发现 (SWIM 协议变体) |
| [sync.rs](src/sync.rs) | 状态同步 (Leader → Follower) |
| [failover.rs](src/failover.rs) | 故障检测与转移 |
| [manager.rs](src/manager.rs) | 统一整合模块 |

## 快速开始

### 创建集群管理器

```rust
use rex_cluster::manager::{ClusterManager, ClusterManagerConfig};

let config = ClusterManagerConfig::default();
let (manager, mut rx) = ClusterManager::new(config)?;

// 启动管理器
manager.start();
```

### 处理节点加入

```rust
use rex_cluster::{NodeInfo, NodeState, NodeId};
use std::net::SocketAddr;

let node_info = NodeInfo {
    node_id: NodeId::new("node-2"),
    listen_addr: "127.0.0.1:9001".parse().unwrap(),
    is_leader: false,
    state: NodeState::Active,
    last_heartbeat: 0,
    term: 0,
    version: 1,
};

manager.handle_node_join(node_info);
```

### 角色管理

```rust
use rex_cluster::ClusterRole;

// 成为 Leader
manager.set_role(ClusterRole::Leader);
assert!(manager.is_leader());

// 降级为 Follower
manager.set_role(ClusterRole::Follower);
```

### 查询节点状态

```rust
// 获取所有节点
let nodes = manager.nodes();

// 获取节点状态
let status = manager.get_node_status("node-2");

// 获取所有节点状态
let all_statuses = manager.get_all_node_statuses();
```

## 配置选项

### ClusterManagerConfig

```rust
use rex_cluster::manager::ClusterManagerConfig;
use rex_cluster::failover::FailoverConfig;

let config = ClusterManagerConfig {
    cluster: ClusterConfig::default(),
    enable_state_sync: true,           // 启用状态同步
    failover_config: FailoverConfig::default(),
};
```

### FailoverConfig

```rust
use rex_cluster::failover::FailoverConfig;

let config = FailoverConfig {
    max_failures: 3,                   // 最大失败次数
    health_check_interval_ms: 5000,    // 健康检查间隔
    suspect_timeout_ms: 15000,        // 疑似超时
    dead_timeout_ms: 30000,           // 死亡超时
};
```

## 集群消息类型

```rust
use rex_cluster::ClusterMessage;

// 节点加入
ClusterMessage::Join(NodeInfo)

// 心跳
ClusterMessage::Heartbeat(HeartbeatMessage)

// Gossip 协议消息
ClusterMessage::Gossip(GossipMessage)

// 消息转发
ClusterMessage::Forward(ForwardMessage)

// 状态同步请求
ClusterMessage::StateSyncRequest(StateSyncRequest)

// 状态同步响应
ClusterMessage::StateSyncResponse(StateSyncResponse)

// 心跳检测
ClusterMessage::Ping
ClusterMessage::Pong
```

## Leader 选举

节点通过 Raft-like 算法进行 Leader 选举：

1. **Standalone** → **Candidate**: 选举超时触发
2. **Candidate** → **Leader**: 获得多数票
3. **Leader** → **Follower**: 发现新 Leader

## 状态同步

Leader 节点定期向 Followers 同步状态：

```rust
// Follower 请求同步
manager.request_sync().await?;

// Leader 广播状态
manager.broadcast_state().await?;
```

## 故障检测

FailoverManager 定期检查节点健康状态：

- **Alive**: 节点正常
- **Suspected**: 疑似故障 (超过 suspect_timeout_ms)
- **Dead**: 确认故障 (超过 dead_timeout_ms)

## 测试

```bash
# 运行所有测试
cargo test -p rex-cluster

# 运行集成测试
cargo test -p rex-cluster --test manager_tests
```

## 与 RexServer 集成

```rust
use rex_server::{RexServerConfig, open_server};

// 启用集群模式
let config = RexServerConfig::default()
    .enable_cluster("127.0.0.1:9000".parse().unwrap())
    .add_seed_node("127.0.0.1:9001".parse().unwrap())
    .set_node_id("node-1".to_string());
```

## CLI 集群测试

### 启动集群节点

```bash
# 启动第一个节点 (作为种子节点)
cargo run --bin rex-cli -- server --address 127.0.0.1:8000 \
    --cluster \
    --cluster-addr 127.0.0.1:9000 \
    --server-id node-1

# 启动第二个节点 (连接种子节点)
cargo run --bin rex-cli -- server --address 127.0.0.1:8001 \
    --cluster \
    --cluster-addr 127.0.0.1:9001 \
    --seeds 127.0.0.1:9000 \
    --server-id node-2

# 启动第三个节点
cargo run --bin rex-cli -- server --address 127.0.0.1:8002 \
    --cluster \
    --cluster-addr 127.0.0.1:9002 \
    --seeds 127.0.0.1:9000 \
    --server-id node-3
```

### 参数说明

| 参数 | 说明 |
|------|------|
| `--address` | 客户端连接监听地址 |
| `--cluster` | 启用集群模式 |
| `--cluster-addr` | 集群节点间通信地址 |
| `--seeds` | 种子节点地址 (逗号分隔) |
| `--server-id` | 服务端 ID |
| `--persist` | 启用持久化 |

### 测试收发

```bash
# 终端1: 启动接收客户端 (连接到节点1)
cargo run --bin rex-cli -- recv --address 127.0.0.1:8000 --titles test

# 终端2: 启动发送客户端 (连接到节点2)
cargo run --bin rex-cli -- bench --address 127.0.0.1:8001 --title test

# 终端3: 启动接收客户端 (连接到节点3)
cargo run --bin rex-cli -- recv --address 127.0.0.1:8002 --titles test
```

### 性能测试

```bash
# 开启 TPS 延迟统计
cargo run --bin rex-cli -- recv --address 127.0.0.1:8000 --titles test --bench

# 发送测试
cargo run --bin rex-cli -- bench --address 127.0.0.1:8001 \
    --title test \
    --typ title \
    --len 1024 \
    --interval 3 \
    --bench
```

### 完整示例：三节点集群测试

```bash
# 终端1: 启动节点1
cargo run --bin rex-cli -- server \
    --address 127.0.0.1:8000 \
    --cluster \
    --cluster-addr 127.0.0.1:9000 \
    --server-id node-1

# 终端2: 启动节点2
cargo run --bin rex-cli -- server \
    --address 127.0.0.1:8001 \
    --cluster \
    --cluster-addr 127.0.0.1:9001 \
    --seeds 127.0.0.1:9000 \
    --server-id node-2

# 终端3: 启动节点3
cargo run --bin rex-cli -- server \
    --address 127.0.0.1:8002 \
    --cluster \
    --cluster-addr 127.0.0.1:9002 \
    --seeds 127.0.0.1:9000 \
    --server-id node-3

# 终端4: 接收端 (连接任意节点)
cargo run --bin rex-cli -- recv \
    --address 127.0.0.1:8000 \
    --titles my-topic

# 终端5: 发送端 (连接到不同节点，测试跨节点路由)
cargo run --bin rex-cli -- bench \
    --address 127.0.0.1:8001 \
    --title my-topic \
    --typ title
```

## 限制

- 小规模集群 (3-5 节点)
- 不支持自动扩缩容
- 需要手动管理节点生命周期

## License

MIT
