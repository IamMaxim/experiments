//! rcc: a redis-rs cluster client for the redis-chaos lab.
//!
//! `load` runs a workload against the cluster, prints a line per second
//! (throughput, latency, errors, split per shard), and on exit reads every key
//! back to count acknowledged writes the cluster lost. redis-rs log lines are
//! printed on the same clock (`RUST_LOG=redis=debug`).

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::Write as _;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use redis::cluster::ClusterClient;
use redis::cluster_async::ClusterConnection;
use redis::cluster_read_routing::RandomReplicaStrategy;
use redis::cluster_routing::{RoutingInfo, SingleNodeRoutingInfo};
use redis::{AsyncCommands, ErrorKind, RedisError, Value};

const DEFAULT_SEEDS: &str = "10.77.11.10:6379,10.77.22.10:6379,10.77.33.10:6379";
/// Designed slot ranges; shard 3 = "fan" for multi-slot commands.
const SHARD_NAMES: [&str; 4] = ["s1", "s2", "s3", "fan"];
const FAN: usize = 3;

static START: OnceLock<Instant> = OnceLock::new();
fn elapsed() -> f64 {
    START.get().map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.0)
}

#[derive(Parser)]
#[command(about = "redis-rs cluster client for the redis-chaos lab")]
struct Cli {
    /// Comma-separated seed nodes.
    #[arg(long, global = true, default_value = DEFAULT_SEEDS)]
    seeds: String,
    #[command(flatten)]
    knobs: Knobs,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Args, Clone)]
struct Knobs {
    /// Per-node connect timeout.
    #[arg(long, global = true, default_value_t = 1000)]
    connect_timeout_ms: u64,
    /// Per-attempt response timeout.
    #[arg(long, global = true, default_value_t = 1000)]
    response_timeout_ms: u64,
    /// Whole-request timeout including retries and redirects; 0 = none.
    #[arg(long, global = true, default_value_t = 3000)]
    overall_timeout_ms: u64,
    /// Leave the overall timeout at the library default (= response timeout).
    #[arg(long, global = true)]
    overall_library_default: bool,
    /// Retries per request (library default 16).
    #[arg(long, global = true, default_value_t = 3)]
    retries: u32,
    /// Set TCP_NODELAY (redis-rs leaves Nagle on by default).
    #[arg(long, global = true)]
    nodelay: bool,
    /// Route reads to a random replica (RandomReplicaStrategy).
    #[arg(long, global = true)]
    read_replicas: bool,
    /// Max in-flight requests per node connection.
    #[arg(long, global = true)]
    concurrency_limit: Option<usize>,
    /// TCP_USER_TIMEOUT: the kernel aborts a connection with unacked data after this long.
    #[arg(long, global = true)]
    tcp_user_timeout_ms: Option<u64>,
    /// TCP keepalive idle time (interval = the same, 3 probes).
    #[arg(long, global = true)]
    keepalive_ms: Option<u64>,
    /// App-side self-healing when a shard keeps failing (see Healer).
    #[arg(long, global = true, value_enum, default_value_t = Heal::Off)]
    self_heal: Heal,
    /// Consecutive failures on one shard that trigger a heal.
    #[arg(long, global = true, default_value_t = 3)]
    heal_after: u32,
    /// Minimum time between two heal actions.
    #[arg(long, global = true, default_value_t = 3000)]
    heal_interval_ms: u64,
}

#[derive(Clone, Copy, ValueEnum, PartialEq, Debug)]
enum Heal {
    Off,
    /// Send a write to a replica of the shard by address: its MOVED makes the
    /// library refresh its slot map.
    Poke,
    /// Build a new ClusterConnection from the seeds and swap it in.
    Rebuild,
    /// Poke first; rebuild if the shard still fails one interval later.
    Both,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show the cluster as the client sees it and round-trip a key per shard.
    Info,
    /// SET a key.
    Set { key: String, value: String },
    /// GET a key.
    Get { key: String },
    /// Run a workload, then verify acknowledged writes survived.
    Load(LoadArgs),
    /// Build a fresh client+connection repeatedly and report how long it takes.
    ConnectProbe {
        #[arg(long, default_value_t = 1000)]
        interval_ms: u64,
        #[arg(long, default_value_t = 10)]
        count: u64,
        /// Connections started at once per tick.
        #[arg(long, default_value_t = 1)]
        parallel: u64,
    },
}

#[derive(Clone, Copy, ValueEnum, PartialEq, Debug)]
enum Mode {
    /// SET one key.
    Set,
    /// GET one key (checks for stale values).
    Get,
    /// SET or GET, by --read-ratio.
    Mixed,
    /// MULTI/EXEC with three SETs on one hash tag.
    Txn,
    /// Non-atomic pipeline of five SETs on one hash tag.
    Pipe,
    /// MSET of three keys that live on three different shards (fan-out).
    Fanout,
}

#[derive(Args)]
struct LoadArgs {
    /// Seconds to run; 0 means until Ctrl-C.
    #[arg(long, default_value_t = 30)]
    duration: u64,
    /// Total target ops per second across workers.
    #[arg(long, default_value_t = 400)]
    rate: u64,
    /// Closed-loop workers; each waits for its request before sending the next.
    #[arg(long, default_value_t = 16)]
    workers: u64,
    /// Distinct keys.
    #[arg(long, default_value_t = 1000)]
    keys: u64,
    #[arg(long, value_enum, default_value_t = Mode::Set)]
    mode: Mode,
    /// Fraction of reads in mixed mode.
    #[arg(long, default_value_t = 0.5)]
    read_ratio: f64,
    /// Open loop: send at the target rate regardless of outstanding requests.
    #[arg(long)]
    open_loop: bool,
    /// Open loop only: requests allowed in flight; beyond that, ops are shed.
    #[arg(long, default_value_t = 512)]
    max_inflight: usize,
    /// Skip the read-back at the end.
    #[arg(long)]
    no_verify: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    START.set(Instant::now()).ok();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("redis=info"))
        .format(|buf, rec| {
            writeln!(buf, "t={:>5.1}s LOG {} {}", elapsed(), rec.level(), rec.args())
        })
        .init();
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Info => info(connect(&cli.seeds, &cli.knobs).await?).await,
        Cmd::Set { key, value } => {
            let mut con = connect(&cli.seeds, &cli.knobs).await?;
            let _: () = con.set(&key, &value).await?;
            println!("OK");
            Ok(())
        }
        Cmd::Get { key } => {
            let mut con = connect(&cli.seeds, &cli.knobs).await?;
            let v: Option<String> = con.get(&key).await?;
            println!("{}", v.as_deref().unwrap_or("(nil)"));
            Ok(())
        }
        Cmd::Load(args) => {
            let t = Instant::now();
            let client = build_client(&cli.seeds, &cli.knobs)?;
            let con = client.get_async_connection().await.context("connecting to cluster")?;
            println!("t={:>5.1}s connected in {:?}", elapsed(), t.elapsed());
            let healer = Healer::new(client, con, &cli.knobs).await;
            load(healer, args, &cli.seeds, &cli.knobs).await
        }
        Cmd::ConnectProbe { interval_ms, count, parallel } => {
            connect_probe(cli.seeds, cli.knobs, interval_ms, count, parallel).await
        }
    }
}

async fn connect(seeds: &str, k: &Knobs) -> Result<ClusterConnection> {
    build_client(seeds, k)?.get_async_connection().await.context("connecting to cluster")
}

fn build_client(seeds: &str, k: &Knobs) -> Result<ClusterClient> {
    let nodes: Vec<String> = seeds.split(',').map(|s| format!("redis://{s}")).collect();
    let mut tcp = redis::io::tcp::TcpSettings::default().set_nodelay(k.nodelay);
    if let Some(ms) = k.tcp_user_timeout_ms {
        tcp = tcp.set_user_timeout(Duration::from_millis(ms));
    }
    if let Some(ms) = k.keepalive_ms {
        let d = Duration::from_millis(ms);
        tcp = tcp.set_keepalive(
            socket2::TcpKeepalive::new().with_time(d).with_interval(d).with_retries(3),
        );
    }
    let mut b = ClusterClient::builder(nodes)
        .connection_timeout(Duration::from_millis(k.connect_timeout_ms))
        .response_timeout(Duration::from_millis(k.response_timeout_ms))
        .retries(k.retries)
        .tcp_settings(tcp);
    if !k.overall_library_default {
        b = b.overall_response_timeout(
            (k.overall_timeout_ms > 0).then(|| Duration::from_millis(k.overall_timeout_ms)),
        );
    }
    if k.read_replicas {
        b = b.read_routing_strategy(RandomReplicaStrategy);
    }
    if let Some(l) = k.concurrency_limit {
        b = b.connection_concurrency_limit(l);
    }
    Ok(b.build()?)
}

// --- app-side self-healing ----------------------------------------------------

/// Probe keys whose slots land on the three designed shards ({b}=3300,
/// {c}=7365, {a}=15495).
const PROBE_TAGS: [&str; 3] = ["b", "c", "a"];

/// Wraps the ClusterConnection so the application can recover from the one
/// thing redis-rs never recovers from on its own: a master that stops
/// answering without closing the socket. Workers take the current handle
/// per operation and report every result; after `heal_after` consecutive
/// failures on one shard the healer pokes a replica (its MOVED makes the
/// library refresh), rebuilds the connection from the seeds, or both.
struct Healer {
    client: ClusterClient,
    conn: RwLock<ClusterConnection>,
    mode: Heal,
    after: u32,
    interval: Duration,
    strikes: [AtomicU32; 4],
    busy: AtomicBool,
    last: Mutex<Option<Instant>>,
    /// (start, end, replicas) per slot range, from CLUSTER SLOTS.
    ranges: Mutex<Vec<(u16, u16, Vec<(String, u16)>)>>,
    pub rebuilds: AtomicU64,
    pub pokes: AtomicU64,
}

impl Healer {
    async fn new(client: ClusterClient, con: ClusterConnection, k: &Knobs) -> Arc<Self> {
        let h = Arc::new(Self {
            client,
            conn: RwLock::new(con),
            mode: k.self_heal,
            after: k.heal_after.max(1),
            interval: Duration::from_millis(k.heal_interval_ms),
            strikes: Default::default(),
            busy: AtomicBool::new(false),
            last: Mutex::new(None),
            ranges: Mutex::new(Vec::new()),
            rebuilds: AtomicU64::new(0),
            pokes: AtomicU64::new(0),
        });
        if h.mode != Heal::Off {
            h.refresh_ranges().await;
            let n: usize = h.ranges.lock().unwrap().iter().map(|r| r.2.len()).sum();
            println!("t={:>5.1}s HEAL mode={:?} after={} interval={:?}; {} replicas known", elapsed(), h.mode, h.after, h.interval, n);
        }
        h
    }

    fn conn(&self) -> ClusterConnection {
        self.conn.read().unwrap().clone()
    }

    /// Errors that mean "this shard is not being served through the current
    /// map", as opposed to application errors.
    fn heal_worthy(e: &RedisError) -> bool {
        match e.kind() {
            ErrorKind::Io | ErrorKind::ClusterConnectionNotFound => true,
            ErrorKind::Server(k) => {
                use redis::ServerErrorKind as S;
                matches!(k, S::Moved | S::Ask | S::ClusterDown | S::TryAgain | S::MasterDown)
                    || (k == S::ExecAbort && e.to_string().contains("Moved"))
            }
            _ => false,
        }
    }

    fn observe(self: &Arc<Self>, shard: usize, res: &redis::RedisResult<bool>) {
        if self.mode == Heal::Off {
            return;
        }
        match res {
            Ok(_) => self.strikes[shard].store(0, Ordering::Relaxed),
            Err(e) if Self::heal_worthy(e) => {
                let n = self.strikes[shard].fetch_add(1, Ordering::Relaxed) + 1;
                if n >= self.after && !self.busy.swap(true, Ordering::AcqRel) {
                    let h = self.clone();
                    tokio::spawn(async move { h.heal(shard, n).await });
                }
            }
            Err(_) => {}
        }
    }

    async fn heal(self: Arc<Self>, shard: usize, strikes: u32) {
        let last = *self.last.lock().unwrap();
        if let Some(t) = last {
            let since = t.elapsed();
            if since < self.interval {
                tokio::time::sleep(self.interval - since).await;
            }
        }
        println!("t={:>5.1}s HEAL {} failing ({strikes} strikes): {:?}", elapsed(), SHARD_NAMES[shard], self.mode);
        let shards: Vec<usize> = if shard == FAN { vec![0, 1, 2] } else { vec![shard] };
        match self.mode {
            Heal::Poke => {
                for s in &shards {
                    self.poke(*s).await;
                }
            }
            Heal::Rebuild => self.rebuild().await,
            Heal::Both => {
                for s in &shards {
                    self.poke(*s).await;
                    self.strikes[*s].store(0, Ordering::Relaxed);
                }
                tokio::time::sleep(self.interval).await;
                let still: Vec<usize> = shards.iter().copied().filter(|s| self.strikes[*s].load(Ordering::Relaxed) >= self.after).collect();
                if !still.is_empty() {
                    println!("t={:>5.1}s HEAL {} still failing after poke; rebuilding", elapsed(), SHARD_NAMES[still[0]]);
                    self.rebuild().await;
                }
            }
            Heal::Off => {}
        }
        for s in shards {
            self.strikes[s].store(0, Ordering::Relaxed);
        }
        *self.last.lock().unwrap() = Some(Instant::now());
        self.busy.store(false, Ordering::Release);
    }

    /// SET through a replica of the shard, addressed explicitly. The replica
    /// answers MOVED; redis-rs follows it and schedules a slot refresh. The
    /// probe's own result does not matter.
    async fn poke(&self, shard: usize) {
        let key = format!("rcc:heal:{{{}}}", PROBE_TAGS[shard]);
        let slot = slot(&key);
        let replicas: Vec<(String, u16)> = self
            .ranges
            .lock()
            .unwrap()
            .iter()
            .find(|(s, e, _)| *s <= slot && slot <= *e)
            .map(|r| r.2.clone())
            .unwrap_or_default();
        if replicas.is_empty() {
            println!("t={:>5.1}s HEAL poke {}: no replica known", elapsed(), SHARD_NAMES[shard]);
            return;
        }
        for (host, port) in replicas {
            let t = Instant::now();
            let mut con = self.conn();
            let mut cmd = redis::cmd("SET");
            cmd.arg(&key).arg(elapsed() as u64);
            let r = con
                .route_command(cmd, RoutingInfo::SingleNode(SingleNodeRoutingInfo::ByAddress { host: host.clone(), port }))
                .await;
            self.pokes.fetch_add(1, Ordering::Relaxed);
            println!(
                "t={:>5.1}s HEAL poke {} via {host}:{port}: {} in {:.0}ms",
                elapsed(),
                SHARD_NAMES[shard],
                match &r {
                    Ok(_) => "ok".to_string(),
                    Err(e) => short_err(e),
                },
                t.elapsed().as_secs_f64() * 1e3
            );
            // One answer from any replica is enough; a timeout means that
            // replica is unreachable too, so try the next one.
            if !matches!(&r, Err(e) if e.is_timeout() || e.kind() == ErrorKind::Io) {
                break;
            }
        }
    }

    async fn rebuild(&self) {
        let t = Instant::now();
        match self.client.get_async_connection().await {
            Ok(con) => {
                *self.conn.write().unwrap() = con;
                self.rebuilds.fetch_add(1, Ordering::Relaxed);
                println!("t={:>5.1}s HEAL rebuilt connection in {:.0}ms", elapsed(), t.elapsed().as_secs_f64() * 1e3);
                self.refresh_ranges().await;
            }
            Err(e) => println!("t={:>5.1}s HEAL rebuild FAILED in {:.0}ms: {e}", elapsed(), t.elapsed().as_secs_f64() * 1e3),
        }
    }

    /// Learn each slot range's replicas from CLUSTER SLOTS (any node).
    async fn refresh_ranges(&self) {
        let mut con = self.conn();
        let r = con
            .route_command(redis::cmd("CLUSTER").arg("SLOTS").clone(), RoutingInfo::SingleNode(SingleNodeRoutingInfo::Random))
            .await;
        let Ok(Value::Array(ranges)) = r else {
            println!("t={:>5.1}s HEAL CLUSTER SLOTS failed: {:?}", elapsed(), r.err().map(|e| short_err(&e)));
            return;
        };
        let mut out = Vec::new();
        for r in ranges {
            let Value::Array(parts) = r else { continue };
            let (Some(Value::Int(s)), Some(Value::Int(e))) = (parts.first(), parts.get(1)) else { continue };
            let mut replicas = Vec::new();
            for node in parts.iter().skip(3) {
                if let Value::Array(n) = node
                    && let (Some(Value::BulkString(h)), Some(Value::Int(p))) = (n.first(), n.get(1))
                {
                    replicas.push((String::from_utf8_lossy(h).to_string(), *p as u16));
                }
            }
            out.push((*s as u16, *e as u16, replicas));
        }
        *self.ranges.lock().unwrap() = out;
    }
}

async fn info(mut con: ClusterConnection) -> Result<()> {
    let nodes: String = redis::cmd("CLUSTER").arg("NODES").query_async(&mut con).await?;
    print!("{nodes}");
    // {a}, {b}, {c} hash to slots 15495, 3300, 7365: one key per shard.
    for tag in ["b", "c", "a"] {
        let key = format!("rcc:info:{{{tag}}}");
        let t = Instant::now();
        let _: () = con.set(&key, "ok").await?;
        let v: String = con.get(&key).await?;
        println!("{key} -> {v} in {:?}", t.elapsed());
    }
    Ok(())
}

async fn connect_probe(seeds: String, k: Knobs, interval_ms: u64, count: u64, parallel: u64) -> Result<()> {
    let mut tick = tokio::time::interval(Duration::from_millis(interval_ms));
    for _ in 0..count {
        tick.tick().await;
        let mut hs = Vec::new();
        for _ in 0..parallel {
            let (seeds, k) = (seeds.clone(), k.clone());
            hs.push(tokio::spawn(async move {
                let t = Instant::now();
                let r = connect(&seeds, &k).await;
                (t.elapsed(), r.map(|_| ()))
            }));
        }
        for h in hs {
            let (d, r) = h.await?;
            match r {
                Ok(()) => println!("t={:>5.1}s connect ok in {:.0}ms", elapsed(), d.as_secs_f64() * 1e3),
                Err(e) => println!("t={:>5.1}s connect FAILED in {:.0}ms: {e:#}", elapsed(), d.as_secs_f64() * 1e3),
            }
        }
    }
    Ok(())
}

// --- load --------------------------------------------------------------------

fn slot(key: &str) -> u16 {
    let k = key.as_bytes();
    let tag = k
        .iter()
        .position(|&b| b == b'{')
        .and_then(|s| k[s + 1..].iter().position(|&b| b == b'}').map(|e| (s, s + 1 + e)))
        .filter(|(s, e)| e > &(s + 1))
        .map(|(s, e)| &k[s + 1..e])
        .unwrap_or(k);
    crc16::State::<crc16::XMODEM>::calculate(tag) % 16384
}

fn shard_of(key: &str) -> usize {
    match slot(key) {
        0..=5460 => 0,
        5461..=10922 => 1,
        _ => 2,
    }
}

#[derive(Default)]
struct Window {
    ok: u64,
    err: u64,
    stale: u64,
    shed: u64,
    lat_us: Vec<u64>,
    errors: BTreeMap<String, u64>,
    shard: [(u64, u64); 4],
}

struct Shared {
    window: Mutex<Window>,
    /// acked[key] = last value the cluster confirmed.
    acked: Mutex<HashMap<String, u64>>,
    /// First full text of each error kind, printed once.
    seen: Mutex<BTreeMap<String, ()>>,
    run: u64,
    mode: Mode,
    read_ratio: f64,
}

/// One operation on key index `k` with sequence `seq`. Returns the shard it
/// was attributed to and the result; `Ok(false)` means a stale read.
async fn one_op(con: &mut ClusterConnection, sh: &Shared, k: u64, seq: u64, is_read: bool) -> (usize, redis::RedisResult<bool>) {
    let run = sh.run;
    match sh.mode {
        Mode::Set | Mode::Get | Mode::Mixed => {
            let key = format!("rcc:{run}:{k}");
            let shard = shard_of(&key);
            if is_read {
                let r: redis::RedisResult<Option<u64>> = con.get(&key).await;
                (shard, r.map(|v| {
                    let want = sh.acked.lock().unwrap().get(&key).copied().unwrap_or(0);
                    v.unwrap_or(0) >= want
                }))
            } else {
                let r: redis::RedisResult<()> = con.set(&key, seq).await;
                if r.is_ok() {
                    sh.acked.lock().unwrap().insert(key, seq);
                }
                (shard, r.map(|_| true))
            }
        }
        Mode::Txn | Mode::Pipe => {
            let tag = format!("rcc:{run}:{{{k}}}");
            let shard = shard_of(&tag);
            let n = if sh.mode == Mode::Txn { 3 } else { 5 };
            let keys: Vec<String> = (0..n).map(|i| format!("{tag}:{i}")).collect();
            let mut p = redis::pipe();
            if sh.mode == Mode::Txn {
                p.atomic();
            }
            for key in &keys {
                p.set(key, seq).ignore();
            }
            let r: redis::RedisResult<()> = p.query_async(con).await;
            if r.is_ok() {
                let mut a = sh.acked.lock().unwrap();
                for key in keys {
                    a.insert(key, seq);
                }
            }
            (shard, r.map(|_| true))
        }
        Mode::Fanout => {
            // {b},{c},{a} land on shards 1,2,3.
            let keys: Vec<String> = ["b", "c", "a"].iter().map(|t| format!("rcc:{run}:{{{t}}}:{k}")).collect();
            let r: redis::RedisResult<()> = con.mset(&keys.iter().map(|key| (key.as_str(), seq)).collect::<Vec<_>>()).await;
            if r.is_ok() {
                let mut a = sh.acked.lock().unwrap();
                for key in keys {
                    a.insert(key, seq);
                }
            }
            (FAN, r.map(|_| true))
        }
    }
}

fn record(sh: &Shared, shard: usize, us: u64, res: redis::RedisResult<bool>) {
    let mut w = sh.window.lock().unwrap();
    match res {
        Ok(fresh) => {
            w.ok += 1;
            w.shard[shard].0 += 1;
            w.lat_us.push(us);
            if !fresh {
                w.stale += 1;
            }
        }
        Err(e) => {
            w.err += 1;
            w.shard[shard].1 += 1;
            let s = short_err(&e);
            *w.errors.entry(s.clone()).or_default() += 1;
            drop(w);
            if sh.seen.lock().unwrap().insert(s.clone(), ()).is_none() {
                eprintln!("t={:>5.1}s first '{s}' on {}: {e}", elapsed(), SHARD_NAMES[shard]);
            }
        }
    }
}

async fn load(healer: Arc<Healer>, args: LoadArgs, seeds: &str, knobs: &Knobs) -> Result<()> {
    let run = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs();
    let sh = Arc::new(Shared {
        window: Mutex::new(Window::default()),
        acked: Mutex::new(HashMap::new()),
        seen: Mutex::new(BTreeMap::new()),
        run,
        mode: args.mode,
        read_ratio: args.read_ratio,
    });
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    println!(
        "t={:>5.1}s run {run}: mode={:?} {} {} ops/s, {} keys, prefix rcc:{run}:",
        elapsed(),
        args.mode,
        if args.open_loop { format!("open-loop (max {} in flight)", args.max_inflight) } else { format!("{} closed-loop workers", args.workers) },
        args.rate,
        args.keys
    );

    let mut handles = Vec::new();
    if args.open_loop {
        let healer = healer.clone();
        let sh = sh.clone();
        let stop = stop_rx.clone();
        let (rate, max_inflight, keys) = (args.rate.max(1), args.max_inflight, args.keys);
        handles.push(tokio::spawn(async move {
            let free: Arc<Mutex<VecDeque<u64>>> = Arc::new(Mutex::new((0..keys).collect()));
            let inflight = Arc::new(tokio::sync::Semaphore::new(max_inflight));
            let mut tick = tokio::time::interval(Duration::from_secs_f64(1.0 / rate as f64));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut seq = 0u64;
            let mut i = 0u64;
            while !*stop.borrow() {
                tick.tick().await;
                seq += 1;
                i += 1;
                let permit = match inflight.clone().try_acquire_owned() {
                    Ok(p) => p,
                    Err(_) => {
                        sh.window.lock().unwrap().shed += 1;
                        continue;
                    }
                };
                let Some(k) = free.lock().unwrap().pop_front() else {
                    sh.window.lock().unwrap().shed += 1;
                    continue;
                };
                let is_read = is_read(&sh, i);
                let (healer, sh, free) = (healer.clone(), sh.clone(), free.clone());
                tokio::spawn(async move {
                    let mut con = healer.conn();
                    let t = Instant::now();
                    let (shard, res) = one_op(&mut con, &sh, k, seq, is_read).await;
                    healer.observe(shard, &res);
                    record(&sh, shard, t.elapsed().as_micros() as u64, res);
                    free.lock().unwrap().push_back(k);
                    drop(permit);
                });
            }
            // Let the stragglers finish before verification.
            let _ = tokio::time::timeout(Duration::from_secs(5), inflight.acquire_many(max_inflight as u32)).await;
        }));
    } else {
        let per_worker = Duration::from_secs_f64(args.workers as f64 / args.rate.max(1) as f64);
        for w in 0..args.workers {
            let healer = healer.clone();
            let sh = sh.clone();
            let stop = stop_rx.clone();
            let owned: Vec<u64> = (w..args.keys).step_by(args.workers as usize).collect();
            handles.push(tokio::spawn(async move {
                let mut tick = tokio::time::interval(per_worker);
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                let mut seq = 0u64;
                let mut i = 0u64;
                while !*stop.borrow() && !owned.is_empty() {
                    tick.tick().await;
                    let k = owned[(i % owned.len() as u64) as usize];
                    i += 1;
                    seq += 1;
                    let is_read = is_read(&sh, i);
                    let mut con = healer.conn();
                    let t = Instant::now();
                    let (shard, res) = one_op(&mut con, &sh, k, seq, is_read).await;
                    healer.observe(shard, &res);
                    record(&sh, shard, t.elapsed().as_micros() as u64, res);
                }
            }));
        }
    }

    let started = Instant::now();
    let mut totals = Totals::default();
    let mut report = tokio::time::interval(Duration::from_secs(1));
    report.tick().await;
    loop {
        if args.duration > 0 && started.elapsed() >= Duration::from_secs(args.duration) {
            break;
        }
        tokio::select! {
            _ = report.tick() => {}
            _ = tokio::signal::ctrl_c() => break,
        }
        let w = std::mem::take(&mut *sh.window.lock().unwrap());
        print_window(w, &mut totals);
    }
    stop_tx.send(true)?;
    for h in handles {
        h.await?;
    }
    let w = std::mem::take(&mut *sh.window.lock().unwrap());
    print_window(w, &mut totals);
    println!(
        "total: {} ok, {} errors, {} stale reads, {} shed, max latency {:.1}ms",
        totals.ok, totals.err, totals.stale, totals.shed, totals.max_us as f64 / 1000.0
    );
    for (i, name) in SHARD_NAMES.iter().enumerate() {
        let s = &totals.shard[i];
        if s.err > 0 {
            println!(
                "{name}: {} errors between t={:.0}s and t={:.0}s ({} seconds with errors, {} of them with no ok)",
                s.err, s.first_err, s.last_err, s.bad_secs, s.dead_secs
            );
        }
    }
    let errs: Vec<String> = totals.errors.iter().map(|(k, v)| format!("{v}x {k}")).collect();
    if !errs.is_empty() {
        println!("errors: {}", errs.join("; "));
    }
    if healer.mode != Heal::Off {
        println!(
            "heal: {} pokes, {} rebuilds",
            healer.pokes.load(Ordering::Relaxed),
            healer.rebuilds.load(Ordering::Relaxed)
        );
    }

    if !args.no_verify {
        let acked = std::mem::take(&mut *sh.acked.lock().unwrap());
        // Verification must read from masters: a replica that is still
        // catching up would report acknowledged writes as lost.
        let con = if knobs.read_replicas {
            connect(seeds, &Knobs { read_replicas: false, ..knobs.clone() }).await?
        } else {
            healer.conn()
        };
        verify(con, &acked).await?;
    }
    Ok(())
}

fn is_read(sh: &Shared, i: u64) -> bool {
    match sh.mode {
        Mode::Get => true,
        Mode::Mixed => (i as f64 * sh.read_ratio).fract() < sh.read_ratio.min(0.999),
        _ => false,
    }
}

#[derive(Default)]
struct ShardTotals {
    err: u64,
    first_err: f64,
    last_err: f64,
    bad_secs: u64,
    dead_secs: u64,
}

#[derive(Default)]
struct Totals {
    ok: u64,
    err: u64,
    stale: u64,
    shed: u64,
    max_us: u64,
    errors: BTreeMap<String, u64>,
    shard: [ShardTotals; 4],
}

fn print_window(mut w: Window, totals: &mut Totals) {
    let t = elapsed();
    w.lat_us.sort_unstable();
    let pct = |p: f64| -> f64 {
        if w.lat_us.is_empty() {
            return 0.0;
        }
        let i = ((w.lat_us.len() - 1) as f64 * p).round() as usize;
        w.lat_us[i] as f64 / 1000.0
    };
    let max = w.lat_us.last().copied().unwrap_or(0);
    totals.ok += w.ok;
    totals.err += w.err;
    totals.stale += w.stale;
    totals.shed += w.shed;
    totals.max_us = totals.max_us.max(max);
    for (k, v) in &w.errors {
        *totals.errors.entry(k.clone()).or_default() += v;
    }
    let mut shards = String::new();
    for (i, (ok, err)) in w.shard.iter().enumerate() {
        if *err > 0 {
            let s = &mut totals.shard[i];
            if s.err == 0 {
                s.first_err = t;
            }
            s.err += err;
            s.last_err = t;
            s.bad_secs += 1;
            if *ok == 0 {
                s.dead_secs += 1;
            }
        }
        if i < FAN || *ok + *err > 0 {
            shards.push_str(&format!(" {}={ok}/{err}", SHARD_NAMES[i]));
        }
    }
    let mut extra = String::new();
    if w.stale > 0 {
        extra.push_str(&format!(" stale={}", w.stale));
    }
    if w.shed > 0 {
        extra.push_str(&format!(" shed={}", w.shed));
    }
    let errs: Vec<String> = w.errors.iter().map(|(k, v)| format!("{v}x {k}")).collect();
    println!(
        "t={t:>5.1}s ok={:<4} err={:<4} p50={:>6.2}ms p99={:>7.2}ms max={:>7.2}ms{shards}{extra} {}",
        w.ok,
        w.err,
        pct(0.5),
        pct(0.99),
        max as f64 / 1000.0,
        errs.join("; ")
    );
}

/// Error kind as a short label, e.g. `io:TimedOut`, `server:ClusterDown`, `Moved`.
fn short_err(e: &RedisError) -> String {
    match e.kind() {
        ErrorKind::Io => {
            let io = std::error::Error::source(e)
                .and_then(|s| s.downcast_ref::<std::io::Error>())
                .map(|io| format!("{:?}", io.kind()));
            match io {
                Some(k) => format!("io:{k}"),
                // General Io errors (e.g. "Failed to create initial connections") carry no source.
                None => format!("io:{}", e.to_string().split(" - ").next().unwrap_or("?").chars().take(40).collect::<String>()),
            }
        }
        ErrorKind::Server(k) => {
            let s = e.to_string();
            // "An error was signalled by the server - ResponseError: OOM ..." -> keep the word after the kind.
            let detail = s.split(": ").nth(1).and_then(|d| d.split_whitespace().next()).unwrap_or("");
            format!("server:{k:?}:{detail}")
        }
        k => format!("{k:?}"),
    }
}

/// Reads every acknowledged key back, 16 keys at a time. A value below the
/// acknowledged one means the cluster confirmed a write and then lost it
/// (e.g. async replication across a failover). Retries until the cluster
/// answers, up to a minute.
async fn verify(con: ClusterConnection, acked: &HashMap<String, u64>) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(60);
    let items: Vec<(String, u64)> = acked.iter().map(|(k, v)| (k.clone(), *v)).collect();
    let mut tasks = Vec::new();
    for chunk in items.chunks(items.len().div_ceil(16).max(1)) {
        let chunk = chunk.to_vec();
        let mut con = con.clone();
        tasks.push(tokio::spawn(async move {
            let (mut intact, mut ahead, mut lost) = (0u64, 0u64, Vec::new());
            for (key, want) in chunk {
                let got: Option<u64> = loop {
                    match con.get(&key).await {
                        Ok(v) => break v,
                        Err(e) if Instant::now() < deadline => {
                            eprintln!("t={:>5.1}s verify {key}: {}; retrying", elapsed(), short_err(&e));
                            tokio::time::sleep(Duration::from_millis(500)).await;
                        }
                        Err(e) => return Err(anyhow::Error::from(e).context("verify gave up")),
                    }
                };
                match got {
                    Some(v) if v == want => intact += 1,
                    // A write that timed out client-side but still landed.
                    Some(v) if v > want => ahead += 1,
                    other => lost.push((key, want, other)),
                }
            }
            Ok((intact, ahead, lost))
        }));
    }
    let (mut intact, mut ahead, mut lost) = (0u64, 0u64, Vec::new());
    for t in tasks {
        let (i, a, l) = t.await??;
        intact += i;
        ahead += a;
        lost.extend(l);
    }
    println!(
        "verify: {} keys intact, {} ahead (unacked write landed), {} LOST acknowledged writes",
        intact,
        ahead,
        lost.len()
    );
    lost.sort();
    for (key, want, got) in lost.iter().take(10) {
        println!("  lost {key}: acked {want}, now {got:?}");
    }
    Ok(())
}
