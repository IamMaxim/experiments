//! Smoke test of the handoff module against the lab: 16 workers SET keys for
//! N seconds, one line per second with ok/err, healer counters at the end.
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use redis_cluster_healer::{Healer, HealerConfig, key_slot};
use redis::AsyncCommands;
use redis::cluster::ClusterClient;

#[tokio::main]
async fn main() {
    env_logger::init();
    let secs: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(60);
    let seeds = ["redis://10.77.11.10:6379", "redis://10.77.22.10:6379", "redis://10.77.33.10:6379"];
    let client = ClusterClient::builder(seeds.to_vec())
        .connection_timeout(Duration::from_secs(1))
        .response_timeout(Duration::from_secs(1))
        .overall_response_timeout(Some(Duration::from_secs(3)))
        .retries(3)
        .tcp_settings(redis::io::tcp::TcpSettings::default().set_nodelay(true))
        .build()
        .unwrap();
    let healer = Healer::start(client, HealerConfig::default()).await.unwrap();
    let ok = Arc::new(AtomicU64::new(0));
    let err = Arc::new(AtomicU64::new(0));
    let start = Instant::now();
    for w in 0..16u64 {
        let (h, ok, err) = (healer.clone(), ok.clone(), err.clone());
        tokio::spawn(async move {
            let mut i = w;
            loop {
                let key = format!("healer-check:{i}");
                let mut con = h.conn();
                let res: redis::RedisResult<()> = con.set(&key, 1).await;
                h.observe(key_slot(key.as_bytes()), res.as_ref().err());
                match res {
                    Ok(()) => ok.fetch_add(1, Ordering::Relaxed),
                    Err(_) => err.fetch_add(1, Ordering::Relaxed),
                };
                i += 16;
                tokio::time::sleep(Duration::from_millis(40)).await;
            }
        });
    }
    for _ in 0..secs {
        tokio::time::sleep(Duration::from_secs(1)).await;
        println!(
            "t={:>3}s ok={} err={}",
            start.elapsed().as_secs(),
            ok.swap(0, Ordering::Relaxed),
            err.swap(0, Ordering::Relaxed)
        );
    }
    println!(
        "pokes={} rebuilds={}",
        healer.pokes.load(Ordering::Relaxed),
        healer.rebuilds.load(Ordering::Relaxed)
    );
}
