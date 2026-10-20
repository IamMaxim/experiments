# Handoff: making a redis-rs cluster client survive network incidents without changing redis-rs

Audience: an agent applying this to an application that uses the async
cluster client of redis-rs (`redis::cluster_async::ClusterConnection`).
Everything here was measured in the lab in this directory's parent
(`experiments/reports/J.md`, `G.md`); numbers are from single runs.

## 1. The problem in one paragraph

redis-rs refreshes its slot map only when a reachable node answers MOVED.
A master that stops answering **without closing the socket** (network
blackhole, one-way partition, SIGSTOP, heavy packet loss, the client's own
uplink cut) keeps receiving every request for its slots, each one timing
out, for as long as the socket stays open. The cluster elects a new master
within `cluster-node-timeout` + a few seconds; the client never uses it. In
the lab a 60s blackhole gave 400 failed requests and a shard outage from
9s to 83s (the socket died 20s *after* the network was healed, on TCP's
retransmit backoff). The same mechanism stops MULTI/EXEC from ever
recovering from a failover, because the MOVED is wrapped in `EXECABORT`
which redis-rs classifies as not retryable.

## 2. What to apply (all three; each is independent)

| step | what | fixes | cost |
|---|---|---|---|
| A | `TCP_USER_TIMEOUT` on the client sockets | silent sockets: blackhole, one-way cut, heavy loss, client uplink down | one builder call |
| B | coherent timeouts and retries on the builder | 17s request latency with the defaults; retries that never run | builder calls |
| C | the `Healer` module (`src/healer.rs` here) | everything A cannot see: SIGSTOP, paused process, stale map after failover, MULTI/EXEC after failover | ~250 lines, one call per request |

Measured effect of C, library alone vs with the healer (errors, shard outage):

| incident | before | after |
|---|---|---|
| master blackholed 60s | 400, 9–83s | 25, 9–15s |
| master SIGSTOPped | 144, 9–33s | 22, 9–14s |
| client cut from all masters, masters moved | 272, 9–57s | 48, 9–17s |
| MULTI/EXEC workload, master killed | 5745, never | 888, 6–15s |
| master kill -9 (control) | 32, 9–16s | 25, 9–15s |

Recovery lands 1–2s after the cluster's own failover, which is the floor.

## 3. Step A: TCP_USER_TIMEOUT

```rust
use std::time::Duration;
use redis::io::tcp::TcpSettings;

let tcp = TcpSettings::default()
    .set_nodelay(true)                                   // Nagle doubles latency on real RTTs
    .set_user_timeout(Duration::from_secs(2));           // kernel aborts a socket with unacked data after 2s
let builder = ClusterClient::builder(seeds).tcp_settings(tcp);
```

Pick the user timeout ≥ the response timeout and ≤ `cluster-node-timeout`.
2s with a 1s response timeout worked. Linux only; on other platforms the
call is a no-op and step C carries the load. Keepalive does **not** help
here (probes are only sent on idle sockets), so do not substitute it.

## 4. Step B: timeouts and retries

```rust
let builder = builder
    .connection_timeout(Duration::from_secs(1))
    .response_timeout(Duration::from_secs(1))            // per attempt
    .overall_response_timeout(Some(Duration::from_secs(3))) // whole request incl. retries
    .retries(3);
```

The library default sets the overall timeout equal to the response timeout,
so the first attempt consumes the budget and the configured retries never
run. With no overall timeout a request to a dead node takes
`(retries + 1) × response_timeout` (17s with the default 16 retries).
3 × response as the overall budget keeps a dead shard's requests failing in
3s; the healer's strike threshold is calibrated to that.

## 5. Step C: the Healer

### 5.1 Copy the module

`src/healer.rs` from this directory. Dependencies: `redis` (features
`tokio-comp`, `cluster-async`), `tokio`, `log`, `crc16`. It compiles against
redis-rs commit 14e710d (the pinned submodule in `../redis-rs`); the public
API it uses (`ClusterConnection::route_command`,
`SingleNodeRoutingInfo::ByAddress`, `ClusterClient::get_async_connection`,
`RedisError::{kind, is_timeout}`, `ServerErrorKind`) has been stable across
recent releases, but re-check on upgrade (`cargo test` in this directory).

### 5.2 Wire it in

Replace the single shared `ClusterConnection` with the healer:

```rust
let healer = Healer::start(client, HealerConfig::default()).await?;   // once, at startup
```

Per request:

```rust
let mut con = healer.conn();                       // cheap clone; take a fresh one per request
let res: RedisResult<()> = con.set(&key, &value).await;
healer.observe(key_slot(key.as_bytes()), res.as_ref().err());
```

Rules that matter:

- **Take `healer.conn()` per request**, never cache it in a struct field or
  a task-local for longer than one operation. A rebuild swaps the handle;
  anything holding the old one keeps talking to the stale map.
- **Call `observe` on every result**, successes included: a success resets
  the strike counter for that slot range. Pass the slot of the key the
  request was routed by. For a pipeline or transaction on one hash tag, the
  slot of the tag. For multi-slot commands (`MGET` across tags), observe
  each slot, or the first one.
- Put the two lines in whatever wrapper the codebase already routes Redis
  calls through (a repository struct, an `async fn redis<T>(f)` helper). If
  there is no such wrapper, add one; retrofitting 200 call sites by hand is
  the wrong move.

### 5.3 What it does

1. `start` opens the connection and reads `CLUSTER SLOTS` to learn each
   slot range's replica addresses.
2. `observe` counts consecutive *heal-worthy* errors per slot range
   (timeouts, I/O errors, `MOVED`/`ASK`/`CLUSTERDOWN`/`TRYAGAIN`/`MASTERDOWN`,
   and `EXECABORT` whose payload contains `Moved`). Application errors
   (`WRONGTYPE`, parse errors) are ignored.
3. At `strikes` (3), one heal runs (never two at once, at most one per
   `interval`):
   - **poke**: `SET <probe_key_prefix>{tag} 1 EX 60` routed by address to a
     replica of the failing range. The replica answers MOVED; redis-rs
     follows the redirect and **schedules a slot refresh on receipt of the
     MOVED**, before the redirected attempt is sent. The probe's own result
     is ignored (it usually times out against the old master). If the
     replica is unreachable too, the next one is tried.
   - **rebuild** (if still failing `interval` later, and `rebuild_fallback`
     is on): `get_async_connection()` again and swap the handle. Needed for
     MULTI/EXEC workloads and for a map so stale that no known replica is
     reachable.
4. After a successful rebuild the replica table is re-read.

### 5.4 Configuration

| field | default | guidance |
|---|---|---|
| `strikes` | 3 | with a 3s overall timeout, 3 strikes ≈ 9s of a dead shard before acting; lower to 2 if the workload is bursty enough that 3 consecutive failures are rare |
| `interval` | 3s | ≥ the overall request timeout; the refresh the poke triggers takes ~1s per unreachable node in the map |
| `rebuild_fallback` | true | turn off only if the application uses pubsub on the cluster connection (subscriptions do not survive a rebuild) |
| `probe_key_prefix` | `_healer:probe` | the poke writes this key with a 60s TTL; pick something the application's key scanning ignores |

Seeds: give the builder **all** cluster nodes, or at least one node per
DC, not just the masters. A rebuild connects to the seeds; the poke does
not need them, but the first connection does (I3 in the reports: all seeds
down means `get_async_connection` fails).

### 5.5 Side effects to know

- During the window between the first strike and the failover the poke
  triggers a refresh roughly once per second, and each refresh **stalls
  every request on every shard for the connect timeout** while redis-rs
  dials the dead node under its write lock (library finding 4). Expect
  p99 ≈ 1–2s on healthy shards for the duration of the incident. This is
  the library's behaviour, not the healer's; the healer makes the window
  end at the failover instead of never.
- A rebuild opens one new connection per node; the old ones close when the
  last in-flight request on them finishes.
- Each poke writes one key. `EX 60` keeps it from accumulating.
- The healer does nothing for a master that is paused (`CLIENT PAUSE`) or
  stuck in a Lua script: the cluster bus stays alive, no failover happens,
  there is no new topology to discover. Only replica reads
  (`read_routing_strategy`) help there, and only for reads.

## 6. Verify

Unit: `cargo test` in this directory (slot hashing, probe-tag generation).

End to end, in the lab (`../README.md` quick start, then):

```sh
cd handoff && cargo build --release          # musl target via .cargo/config.toml
docker cp target/x86_64-unknown-linux-musl/release/healer-smoke rc-client:/tmp/
docker exec -e RUST_LOG=redis_cluster_healer=info rc-client /tmp/healer-smoke 45 &
sleep 5; ../bin/chaos isolate dc1-s1; sleep 42; ../bin/chaos heal
```

Expected: ~16 errors at t≈9s, two `poke` log lines, shard back by t≈15s
while dc1-s1 is still isolated, `pokes=2 rebuilds=0` at the end. Without
the healer the same run shows errors every second until well after the
heal. Run `../bin/chaos rebalance` afterwards.

In the target codebase, if a staging cluster is available: SIGSTOP one
master (`kill -STOP`) under load. Recovery within `cluster-node-timeout`
+ ~10s proves the wiring; the latency bump on other shards during that
window is expected.

## 7. What this does not replace

Library fixes are still needed for: the lock stall during refresh, MOVED
and `broken pipe` surfacing to callers when retries are spent on a dead
redirect target, the ≥1.28s CLUSTERDOWN backoff, and EXECABORT handling.
See `../experiments/SUMMARY.md`, findings 4–7. This handoff is the
stop-gap until those land upstream.
