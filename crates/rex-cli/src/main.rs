#![allow(clippy::unwrap_used)]
#![allow(clippy::expect_used)]
#![allow(clippy::panic)]
use std::{
    hint::spin_loop,
    net::SocketAddr,
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow};
use clap::Parser;
use hdrhistogram::Histogram;
use rand::{Rng, distr::Alphanumeric, rng};
use rex_client::{RexClientConfig, RexClientHandlerTrait, open_client};
use rex_core::{Protocol, RexClientInner, RexCommand, RexData, utils::now_micros};
use rex_server::{RexServerConfig, RexSystem, RexSystemConfig, open_server};

#[derive(clap::Parser)]
#[command(
    name = "rex-cli",
    version = "0.1.0",
    author = "ShiLu1211",
    about = "rex_mq"
)]
#[command(propagate_version = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(clap::Subcommand)]
pub enum Commands {
    Server(ServerArgs),
    Recv(RecvArgs),
    Bench(BenchArgs),
}

#[derive(clap::Args)]
#[command(about = "rex server")]
pub struct ServerArgs {
    /// ip:port
    #[arg(short, long)]
    address: String,
    /// 监听协议
    #[arg(short, long, value_parser=["tcp", "quic", "websocket"], default_value = "tcp")]
    protocol: String,
    /// 服务端id
    #[arg(short, long, default_value = "rexd")]
    server_id: String,
}

#[derive(clap::Args)]
#[command(about = "rex recv client")]
pub struct RecvArgs {
    /// 服务端地址
    #[arg(short, long)]
    address: String,
    /// 接收title, 多个用;隔开
    #[arg(short, long)]
    titles: String,
    /// 协议
    #[arg(short, long, value_parser=["tcp", "quic", "websocket"], default_value = "tcp")]
    protocol: String,
    /// 是否开启tps延迟打印, 不开启则打印接收到的内容
    #[arg(short, long, default_value_t = false)]
    bench: bool,
}

#[derive(clap::Args)]
#[command(about = "lgx bench client")]
pub struct BenchArgs {
    /// 服务端地址
    #[arg(short, long)]
    address: String,
    /// 发送类型 A单播 P组播 C广播
    #[arg(short='y', long, value_parser=["title", "group", "cast"], default_value="title")]
    typ: String,
    /// 发送title
    #[arg(short, long)]
    title: String,
    /// 协议
    #[arg(short, long, value_parser=["tcp", "quic", "websocket"], default_value = "tcp")]
    protocol: String,
    // 每次发送长度(>16), 默认1024
    #[arg(short, long, default_value_t = 1024)]
    len: usize,
    // 发送间隔(微秒), 默认3微秒
    #[arg(short, long, default_value_t = 3)]
    interval: u128,
    // 发送模式 消息1,2,3递增 对应recv bench=false的情况
    #[arg(short, long, default_value_t = false)]
    bench: bool,
}

pub async fn start_server(args: ServerArgs) -> Result<()> {
    let address = args.address.parse::<SocketAddr>()?;
    let protocol = Protocol::from(args.protocol.as_str())
        .ok_or_else(|| anyhow!("invalid protocol: {}", args.protocol))?;

    let config = RexServerConfig::new(protocol, address);
    let system = RexSystem::new(RexSystemConfig::from_id(&args.server_id));
    let _server = open_server(system, config).await?;

    loop {
        tokio::time::sleep(Duration::from_millis(1000)).await;
    }
}

pub async fn start_recv(args: RecvArgs) -> Result<()> {
    let address = args.address.parse::<SocketAddr>()?;
    let protocol = Protocol::from(args.protocol.as_str())
        .ok_or_else(|| anyhow!("invalid protocol: {}", args.protocol))?;

    let config = RexClientConfig::new(
        protocol,
        address,
        &args.titles,
        Arc::new(RcvClientHandler::new(args.bench)),
    );
    let _client = open_client(config).await?;

    if args.bench {
        disp_metric().await;
    } else {
        loop {
            tokio::time::sleep(Duration::from_millis(1000)).await;
        }
    }
    Ok(())
}

pub async fn start_bench(args: BenchArgs) -> Result<()> {
    let address = args.address.parse::<SocketAddr>()?;
    let protocol = Protocol::from(args.protocol.as_str())
        .ok_or_else(|| anyhow!("invalid protocol: {}", args.protocol))?;

    let config = RexClientConfig::new(protocol, address, "", Arc::new(SndClientHandler));
    let client = open_client(config).await?;

    let command = match args.typ.as_str() {
        "title" => RexCommand::Title,
        "group" => RexCommand::Group,
        "cast" => RexCommand::Cast,
        _ => return Err(anyhow!("invalid send type: {}", args.typ)),
    };
    let title = args.title;

    let buf: Vec<u8> = rng().sample_iter(&Alphanumeric).take(args.len).collect();
    let mut data = RexData::new(command, &title, &buf);
    let mut cnt = 0;

    loop {
        let now = Instant::now();
        cnt += 1;
        if args.bench {
            data.data_mut()[0..TS_LEN].copy_from_slice(&now_micros().to_be_bytes());
        } else {
            data.set_data(cnt.to_string().as_bytes());
        };

        if let Err(e) = client.send_data(&mut data).await {
            eprintln!("send data error: {}", e);
        }

        while now.elapsed().as_micros() < args.interval {
            spin_loop();
        }
    }
}

#[tokio::main]
async fn main() {
    #[cfg(debug_assertions)]
    {
        tracing_subscriber::fmt::init();
    }
    let cli = Cli::parse();

    if let Some(subcommand) = cli.command {
        let result = match subcommand {
            Commands::Server(args) => start_server(args).await,
            Commands::Recv(args) => start_recv(args).await,
            Commands::Bench(args) => start_bench(args).await,
        };

        if let Err(e) = result {
            eprintln!("Application error: {:#}", e);
            std::process::exit(1);
        }
    }
}

static METRIC: LazyLock<Mutex<Histogram<u64>>> =
    LazyLock::new(|| Mutex::new(Histogram::<u64>::new(3).expect("create histogram failed")));

const TS_LEN: usize = 16;

fn timestamp(data: &[u8]) -> Option<u128> {
    if data.len() < TS_LEN {
        return None;
    }

    let mut buf = [0u8; TS_LEN];
    buf.copy_from_slice(&data[..TS_LEN]);
    Some(u128::from_be_bytes(buf))
}

async fn disp_metric() {
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;

        let mut hist = match METRIC.lock() {
            Ok(h) => h,
            Err(e) => {
                eprintln!("Failed to lock METRIC: {}", e);
                continue;
            }
        };

        if hist.is_empty() {
            println!("tps:0");
            hist.reset();
            continue;
        }

        println!(
            "tps:{} mean:{:.3} min:{} p50:{} p90:{} p95:{} p98:{} p99:{} max:{}",
            hist.len(),
            hist.mean(),
            hist.min(),
            hist.value_at_quantile(0.50),
            hist.value_at_quantile(0.90),
            hist.value_at_quantile(0.95),
            hist.value_at_quantile(0.98),
            hist.value_at_quantile(0.99),
            hist.max(),
        );

        hist.reset();
    }
}

struct RcvClientHandler {
    pub bench: bool,
}

impl RcvClientHandler {
    pub fn new(bench: bool) -> Self {
        Self { bench }
    }
}

#[async_trait::async_trait]
impl RexClientHandlerTrait for RcvClientHandler {
    async fn login_ok(&self, client: Arc<RexClientInner>, _data: RexData) -> Result<()> {
        println!("recv client login ok: [{:032X}]", client.id());
        Ok(())
    }

    async fn handle(&self, _client: Arc<RexClientInner>, data: RexData) -> Result<()> {
        let now = now_micros();
        if self.bench {
            let command = data.command();
            if command == RexCommand::Title
                || command == RexCommand::Group
                || command == RexCommand::Cast
            {
                let Some(ts) = timestamp(data.data()) else {
                    eprintln!("cannot get timestamp from data");
                    return Ok(());
                };

                // 使用 saturating_sub 防止时间回拨崩溃
                let latency = now.saturating_sub(ts);

                let mut hist = match METRIC.lock() {
                    Ok(h) => h,
                    Err(e) => {
                        eprintln!("Failed to lock METRIC: {}", e);
                        return Ok(());
                    }
                };

                if let Err(e) = hist.record(latency as u64) {
                    eprintln!("record error: {}", e);
                }
            }
        } else {
            println!("recv: {}", data.data_as_string_lossy());
        }
        Ok(())
    }
}

struct SndClientHandler;

#[async_trait::async_trait]
impl RexClientHandlerTrait for SndClientHandler {
    async fn login_ok(&self, client: Arc<RexClientInner>, _data: RexData) -> Result<()> {
        println!("send client login ok: [{:032X}]", client.id());
        Ok(())
    }

    async fn handle(&self, _client: Arc<RexClientInner>, data: RexData) -> Result<()> {
        println!("send received: {}", data.data_as_string_lossy());
        Ok(())
    }
}
