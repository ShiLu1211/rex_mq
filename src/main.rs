use std::{
    net::SocketAddr,
    sync::{Arc, LazyLock},
    time::{Duration, Instant},
};

use anyhow::Result;
use bytes::{Bytes, BytesMut};
use clap::Parser;
use hdrhistogram::Histogram;
use rand::{Rng, distr::Alphanumeric, rng};
use tokio::{sync::Mutex, time::sleep};

use rex_mq::{
    RexClient, RexClientConfig, RexClientHandler, RexClientInner, RexServerConfig, RexSystem,
    RexSystemConfig, TcpClient, TcpServer,
    protocol::{RexCommand, RexData},
    utils::{now_micros, timestamp, timestamp_data},
};

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
    Send(SendArgs),
    Bench(BenchArgs),
}

#[derive(clap::Args)]
#[command(about = "rex server")]
pub struct ServerArgs {
    /// ip:port
    #[arg(short, long)]
    address: String,
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
    /// 是否开启tps延迟打印, 不开启则打印接收到的内容
    #[arg(short, long, default_value_t = false)]
    bench: bool,
}

#[derive(clap::Args)]
#[command(about = "rex send client")]
pub struct SendArgs {
    /// 服务端地址
    #[arg(short, long)]
    address: String,
    /// 发送类型 A单播 P组播 C广播
    #[arg(short='y', long, value_parser=["title", "group", "cast"], default_value="title")]
    typ: String,
    /// 发送title
    #[arg(short, long)]
    title: String,
    /// 发送内容
    #[arg(short, long)]
    content: String,
    /// 发送次数, 默认发送1次
    #[arg(short = 'u', long, default_value_t = 1)]
    count: usize,
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
    let config = RexServerConfig::from_addr(address);
    let system = RexSystem::new(RexSystemConfig::from_id(&args.server_id));
    let _server = TcpServer::open(system, config).await?;

    loop {
        sleep(Duration::from_millis(1000)).await;
    }
}

pub async fn start_recv(args: RecvArgs) -> Result<()> {
    let address = args.address.parse::<SocketAddr>()?;
    let config = RexClientConfig::new(
        address,
        args.titles,
        Arc::new(RcvClientHandler::new(args.bench)),
    );
    let client = TcpClient::new(config)?;
    let _client = client.open().await?;

    if args.bench {
        disp_metric().await;
    } else {
        loop {
            sleep(Duration::from_millis(1000)).await;
        }
    }
    Ok(())
}

pub async fn start_send(args: SendArgs) -> Result<()> {
    let address = args.address.parse::<SocketAddr>()?;
    let config = RexClientConfig::new(address, "".to_string(), Arc::new(SndClientHandler));
    let client = TcpClient::new(config)?;
    let _client = client.open().await?;
    Ok(())
}

pub async fn start_bench(args: BenchArgs) -> Result<()> {
    let address = args.address.parse::<SocketAddr>()?;
    let config = RexClientConfig::new(address, "".to_string(), Arc::new(SndClientHandler));
    let client = TcpClient::new(config)?;
    let client = client.open().await?;

    let command = match args.typ.as_str() {
        "title" => RexCommand::Title,
        "group" => RexCommand::Group,
        "cast" => RexCommand::Cast,
        _ => panic!("invalid type"),
    };
    let title = args.title;

    let buf: Vec<u8> = rng().sample_iter(&Alphanumeric).take(args.len).collect();
    let mut cnt = 0;

    loop {
        let now = Instant::now();
        let msg = timestamp_data(buf.clone());
        cnt += 1;
        let mut data = if args.bench {
            let msg_bytes = Bytes::from(msg);
            let msg_bytesmut = BytesMut::from(msg_bytes);
            RexData::builder(command)
                .title(title.clone())
                .data(msg_bytesmut)
                .build()
        } else {
            RexData::builder(command)
                .title(title.clone())
                .data(cnt.to_string().as_bytes().into())
                .build()
        };

        if let Err(e) = client.send_data(&mut data).await {
            eprintln!("send data error: {}", e);
        }

        loop {
            if now.elapsed().as_micros() > args.interval {
                break;
            }
        }
    }
}

#[tokio::main]
async fn main() {
    // 初始化日志
    #[cfg(debug_assertions)]
    {
        tracing_subscriber::fmt::init();
    }
    let cli = Cli::parse();

    if let Some(subcommand) = cli.command {
        let _ = match subcommand {
            Commands::Server(args) => {
                let _ = start_server(args).await;
                Ok(())
            }
            Commands::Recv(args) => start_recv(args).await,
            Commands::Send(args) => start_send(args).await,
            Commands::Bench(args) => start_bench(args).await,
        };
    }
}

static METRIC: LazyLock<Mutex<Histogram<u64>>> =
    LazyLock::new(|| Mutex::new(Histogram::<u64>::new(3).unwrap()));

pub async fn disp_metric() {
    loop {
        sleep(Duration::from_secs(1)).await;
        let hist = {
            let record = METRIC.lock().await;
            record.clone()
        };
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
        {
            let mut hist = METRIC.lock().await;
            hist.reset();
        }
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
impl RexClientHandler for RcvClientHandler {
    async fn login_ok(&self, client: Arc<RexClientInner>, _data: RexData) -> Result<()> {
        println!("recv client login ok: [{:032X}]", client.id());
        Ok(())
    }

    async fn handle(&self, _client: Arc<RexClientInner>, data: RexData) -> Result<()> {
        if self.bench {
            let command = data.header().command();
            if command == RexCommand::Title
                || command == RexCommand::Group
                || command == RexCommand::Cast
            {
                let now = now_micros();
                let latency = now - timestamp(data.data());
                let mut record = METRIC.lock().await;
                record.record(latency as u64).unwrap();
            }
        } else {
            println!("recv: {}", data.data_as_string_lossy());
        }
        Ok(())
    }
}

struct SndClientHandler;

#[async_trait::async_trait]
impl RexClientHandler for SndClientHandler {
    async fn login_ok(&self, client: Arc<RexClientInner>, _data: RexData) -> Result<()> {
        println!("send client login ok: [{:032X}]", client.id());
        Ok(())
    }

    async fn handle(&self, _client: Arc<RexClientInner>, data: RexData) -> Result<()> {
        println!("send received: {}", data.data_as_string_lossy());
        Ok(())
    }
}
