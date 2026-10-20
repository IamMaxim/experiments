//! App-side self-healing for redis-rs `ClusterConnection` (async).
//!
//! redis-rs (as of 14e710d) only refreshes its slot map when a reachable
//! node answers MOVED. A master that stops answering without closing the
//! socket (network blackhole, SIGSTOP, one-way partition, heavy loss) keeps
//! receiving requests forever, even after the cluster elected a new master.
//! This module makes the application notice and force a refresh.
//!
//! Usage:
//!   let healer = Healer::start(client, HealerConfig::default()).await?;
//!   let mut con = healer.conn();                 // per request, cheap clone
//!   let res: RedisResult<()> = con.set(key, v).await;
//!   healer.observe(key_slot(key), res.as_ref().err());
//!
//! See ../HANDOFF.md for the rationale and measurements.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use redis::cluster::ClusterClient;
use redis::cluster_async::ClusterConnection;
use redis::cluster_routing::{RoutingInfo, SingleNodeRoutingInfo};
use redis::{ErrorKind, RedisError, RedisResult, ServerErrorKind, Value};

/// Hash slot of a key (Redis Cluster CRC16 with hash-tag rules).
pub fn key_slot(key: &[u8]) -> u16 {
    let tag = key
        .iter()
        .position(|&b| b == b'{')
        .and_then(|s| key[s + 1..].iter().position(|&b| b == b'}').map(|e| (s, s + 1 + e)))
        .filter(|(s, e)| *e > s + 1)
        .map(|(s, e)| &key[s + 1..e])
        .unwrap_or(key);
    crc16::State::<crc16::XMODEM>::calculate(tag) % 16384
}

#[derive(Clone, Debug)]
pub struct HealerConfig {
    /// Consecutive heal-worthy failures on one slot range before acting.
    /// 3 with a 3s overall request timeout means ~9s of a dead shard.
    pub strikes: u32,
    /// Minimum gap between two heal actions.
    pub interval: Duration,
    /// Rebuild the connection if the range still fails `interval` after a poke.
    /// Needed for MULTI/EXEC workloads (EXECABORT never triggers a refresh).
    pub rebuild_fallback: bool,
    /// Key written by the poke. Must hash into the failing range, so the
    /// healer appends a hash tag for a slot of that range to this prefix.
    pub probe_key_prefix: String,
}

impl Default for HealerConfig {
    fn default() -> Self {
        Self {
            strikes: 3,
            interval: Duration::from_secs(3),
            rebuild_fallback: true,
            probe_key_prefix: "_healer:probe".to_string(),
        }
    }
}

/// One entry of CLUSTER SLOTS: slot range and replica addresses.
#[derive(Clone, Debug)]
struct Range {
    start: u16,
    end: u16,
    replicas: Vec<(String, u16)>,
}

pub struct Healer {
    client: ClusterClient,
    conn: RwLock<ClusterConnection>,
    cfg: HealerConfig,
    ranges: Mutex<Vec<Range>>,
    /// Consecutive failures per range index (index into `ranges`).
    strikes: Mutex<Vec<AtomicU32>>,
    busy: AtomicBool,
    last_heal: Mutex<Option<Instant>>,
    pub pokes: AtomicU64,
    pub rebuilds: AtomicU64,
}

impl Healer {
    /// Opens the first connection and learns the slot ranges.
    pub async fn start(client: ClusterClient, cfg: HealerConfig) -> RedisResult<Arc<Self>> {
        let conn = client.get_async_connection().await?;
        let h = Arc::new(Self {
            client,
            conn: RwLock::new(conn),
            cfg,
            ranges: Mutex::new(Vec::new()),
            strikes: Mutex::new(Vec::new()),
            busy: AtomicBool::new(false),
            last_heal: Mutex::new(None),
            pokes: AtomicU64::new(0),
            rebuilds: AtomicU64::new(0),
        });
        h.refresh_ranges().await;
        Ok(h)
    }

    /// The current connection handle. Take one per request; do not cache it
    /// across requests or a rebuild will not reach you.
    pub fn conn(&self) -> ClusterConnection {
        self.conn.read().unwrap().clone()
    }

    /// Errors that mean "this slot is not served through the current map".
    /// Application-level errors (WRONGTYPE, type conversion...) are ignored.
    pub fn heal_worthy(e: &RedisError) -> bool {
        match e.kind() {
            ErrorKind::Io | ErrorKind::ClusterConnectionNotFound => true,
            ErrorKind::Server(k) => {
                matches!(
                    k,
                    ServerErrorKind::Moved
                        | ServerErrorKind::Ask
                        | ServerErrorKind::ClusterDown
                        | ServerErrorKind::TryAgain
                        | ServerErrorKind::MasterDown
                ) || (k == ServerErrorKind::ExecAbort && e.to_string().contains("Moved"))
            }
            _ => false,
        }
    }

    /// Report the outcome of a request on `slot`. `None` on success.
    pub fn observe(self: &Arc<Self>, slot: u16, err: Option<&RedisError>) {
        let Some(idx) = self.range_index(slot) else { return };
        let strikes = self.strikes.lock().unwrap();
        let Some(counter) = strikes.get(idx) else { return };
        match err {
            None => counter.store(0, Ordering::Relaxed),
            Some(e) if Self::heal_worthy(e) => {
                let n = counter.fetch_add(1, Ordering::Relaxed) + 1;
                if n >= self.cfg.strikes && !self.busy.swap(true, Ordering::AcqRel) {
                    let h = self.clone();
                    tokio::spawn(async move { h.heal(idx, n).await });
                }
            }
            Some(_) => {}
        }
    }

    fn range_index(&self, slot: u16) -> Option<usize> {
        self.ranges.lock().unwrap().iter().position(|r| r.start <= slot && slot <= r.end)
    }

    async fn heal(self: Arc<Self>, idx: usize, strikes: u32) {
        // Copy out: a MutexGuard must not live across the await below.
        let last = *self.last_heal.lock().unwrap();
        if let Some(t) = last {
            let since = t.elapsed();
            if since < self.cfg.interval {
                tokio::time::sleep(self.cfg.interval - since).await;
            }
        }
        let range = self.ranges.lock().unwrap().get(idx).cloned();
        log::warn!("healer: range {idx} failing ({strikes} strikes); poking");
        if let Some(r) = &range {
            self.poke(r).await;
        }
        self.reset(idx);
        if self.cfg.rebuild_fallback {
            tokio::time::sleep(self.cfg.interval).await;
            let still = self.strikes.lock().unwrap().get(idx).map(|c| c.load(Ordering::Relaxed)).unwrap_or(0);
            if still >= self.cfg.strikes || range.is_none() {
                log::warn!("healer: range {idx} still failing after poke; rebuilding connection");
                self.rebuild().await;
            }
        }
        self.reset(idx);
        *self.last_heal.lock().unwrap() = Some(Instant::now());
        self.busy.store(false, Ordering::Release);
    }

    fn reset(&self, idx: usize) {
        if let Some(c) = self.strikes.lock().unwrap().get(idx) {
            c.store(0, Ordering::Relaxed);
        }
    }

    /// SET a probe key through a replica of the range, addressed explicitly.
    /// The replica answers MOVED; redis-rs follows it and schedules a slot
    /// refresh on receipt. The probe's own result is irrelevant.
    async fn poke(&self, r: &Range) {
        if r.replicas.is_empty() {
            log::warn!("healer: no replica known for slots {}-{}", r.start, r.end);
            return;
        }
        let key = format!("{}{{{}}}", self.cfg.probe_key_prefix, tag_for_slot(r.start));
        for (host, port) in &r.replicas {
            let mut con = self.conn();
            let mut cmd = redis::cmd("SET");
            cmd.arg(&key).arg(1).arg("EX").arg(60);
            let t = Instant::now();
            let res = con
                .route_command(
                    cmd,
                    RoutingInfo::SingleNode(SingleNodeRoutingInfo::ByAddress { host: host.clone(), port: *port }),
                )
                .await;
            self.pokes.fetch_add(1, Ordering::Relaxed);
            log::info!("healer: poke via {host}:{port} -> {:?} in {:?}", res.as_ref().map(|_| ()), t.elapsed());
            // A timeout or I/O error means this replica is unreachable too; try the next.
            if !matches!(&res, Err(e) if e.is_timeout() || e.kind() == ErrorKind::Io) {
                break;
            }
        }
    }

    /// Fresh connection from the seeds, swapped in for new requests. Requests
    /// already in flight finish on the old handle. Pubsub subscriptions on
    /// the old connection are lost.
    async fn rebuild(&self) {
        let t = Instant::now();
        match self.client.get_async_connection().await {
            Ok(c) => {
                *self.conn.write().unwrap() = c;
                self.rebuilds.fetch_add(1, Ordering::Relaxed);
                log::warn!("healer: rebuilt connection in {:?}", t.elapsed());
                self.refresh_ranges().await;
            }
            Err(e) => log::error!("healer: rebuild failed in {:?}: {e}", t.elapsed()),
        }
    }

    /// Learn slot ranges and their replicas from CLUSTER SLOTS (any node).
    async fn refresh_ranges(&self) {
        let mut con = self.conn();
        let mut cmd = redis::cmd("CLUSTER");
        cmd.arg("SLOTS");
        let res = con.route_command(cmd, RoutingInfo::SingleNode(SingleNodeRoutingInfo::Random)).await;
        let Ok(Value::Array(entries)) = res else {
            log::warn!("healer: CLUSTER SLOTS failed: {:?}", res.err());
            return;
        };
        let mut out = Vec::new();
        for e in entries {
            let Value::Array(parts) = e else { continue };
            let (Some(Value::Int(s)), Some(Value::Int(end))) = (parts.first(), parts.get(1)) else { continue };
            let mut replicas = Vec::new();
            // parts[2] is the master; the rest are replicas.
            for node in parts.iter().skip(3) {
                if let Value::Array(n) = node
                    && let (Some(Value::BulkString(h)), Some(Value::Int(p))) = (n.first(), n.get(1))
                {
                    replicas.push((String::from_utf8_lossy(h).to_string(), *p as u16));
                }
            }
            out.push(Range { start: *s as u16, end: *end as u16, replicas });
        }
        let n = out.len();
        *self.strikes.lock().unwrap() = (0..n).map(|_| AtomicU32::new(0)).collect();
        *self.ranges.lock().unwrap() = out;
    }
}

/// A hash tag whose slot is exactly `slot`: brute-forced once per call (a
/// few hundred microseconds), used only when building a probe key.
fn tag_for_slot(slot: u16) -> String {
    for i in 0u32.. {
        let tag = i.to_string();
        if key_slot(tag.as_bytes()) == slot {
            return tag;
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_match_redis() {
        assert_eq!(key_slot(b"foo"), 12182);
        assert_eq!(key_slot(b"{user1000}.following"), key_slot(b"user1000"));
        assert_eq!(key_slot(b"{}x"), key_slot(b"{}x"));
    }

    #[test]
    fn tag_lands_on_slot() {
        for s in [0u16, 5461, 10923, 16383] {
            let tag = tag_for_slot(s);
            assert_eq!(key_slot(format!("p{{{tag}}}").as_bytes()), s);
        }
    }
}
