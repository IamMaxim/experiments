# Experiment plan

Goal: observe how redis-rs (async cluster client, commit 14e710d) behaves under
every failure mode a Redis Cluster can present, record what the cluster did and
what the client did, and collect the evidence for library changes.

Each experiment runs under a steady write (or mixed) load from `rcc load` and
records, per second and per shard, ok/err counts, latency, and the error
kinds. Status snapshots of all 9 nodes are taken every 2s from the router. The
client runs with `RUST_LOG=redis=debug`, so slot refreshes and reconnects in
the library appear with timestamps next to the load lines.

Default client knobs unless an experiment says otherwise: connect 1s, response
1s, overall 3s, 3 retries, `--nodelay`, 16 closed-loop workers, 400 ops/s.

Reports go to `experiments/reports/<id>.md`; raw runs to `experiments/runs/`.

## What the library does today (from the code)

| Trigger | redis-rs reaction |
|---|---|
| Response timeout | `RetryImmediately`: same slot map, same node, until retries or overall timeout run out. No slot refresh. |
| Connection reset/refused/EOF | `Reconnect(addr)` + retry; while the node is `Reconnecting`, requests fall back to another node of the same shard (a replica, which answers MOVED for writes, which triggers a slot refresh). |
| MOVED | redirect + slot refresh. ASK: redirect only. |
| READONLY | slot refresh + retry. |
| CLUSTERDOWN, TRYAGAIN, MASTERDOWN, LOADING | sleep (min 1.28s, exponential, jittered) + retry on the same node. No refresh. |
| No connection at all | reconnect from the seed list + full refresh. |
| Anything else (OOM, WRONGTYPE, EXECABORT, CROSSSLOT, fan-out errors) | returned as is. |
| Time, with nothing failing | nothing: no periodic topology refresh, no keepalive, no TCP user timeout by default. |

## Matrix

Columns: what the cluster should do; what we expect the client to do; what we
measure beyond the standard ok/err/latency/lost.

### A. One master fails

| id | incident | cluster | expected client | watch |
|---|---|---|---|---|
| A1 | blackhole master `dc1-s1`, 60s | failover ~6s | stuck on shard 1 until heal (timeouts never refresh) | does it ever recover on its own; after heal, how long until the rejoined replica stops taking writes |
| A2 | same, ICMP reject | same | same for existing connections (ICMP is a soft error on an established TCP socket) | whether new connects fail fast and what error kind they map to |
| A3 | kill -9 master, leave it dead 60s | failover ~6s | EOF → reconnect loop → fallback to replica → MOVED → refresh; recovers in seconds | reconnect loop backoff, errors per second while the node is dead |
| A4 | SIGSTOP master (hung process), 60s | failover ~6s | as A1: the kernel still ACKs, so no error, only timeouts | same as A1 |
| A5 | graceful `docker stop` (SIGTERM), start after 30s | failover; old master returns as replica | server closes connections → EOF → as A3 | errors during the shutdown instant |
| A6 | `docker restart` master (back in ~2s, inside node-timeout) | no failover; node returns as master | reconnect, a few errors | no slot refresh needed; duration of outage |
| A7 | manual `CLUSTER FAILOVER` on a replica | coordinated, no lost writes | MOVED → refresh; ~0 errors | error count; whether any write is lost |
| A8 | `CLIENT PAUSE 30000 WRITE` on master (slow node, bus alive) | no failover | every write to shard 1 times out for 30s; reads work | nothing in the client can help; do timeouts pile up? |
| A9 | `DEBUG SLEEP 15` on master (blocks the bus too) | failover after ~6s, old master wakes as master briefly, then gets demoted | timeouts, then MOVED | behaviour in the double-master moment |

### B. One replica fails

| id | incident | cluster | expected client | watch |
|---|---|---|---|---|
| B1 | blackhole replica `dc2-s1`, master-only reads | nothing user-visible | 0 errors | background refresh/reconnect noise |
| B2 | same, with `--read-replicas` (RandomReplicaStrategy) | nothing | reads to the dead replica time out; the client never removes it from the read candidates | fraction of failing reads; whether a refresh drops the FAIL replica from CLUSTER SLOTS |
| B3 | kill replica, with `--read-replicas` | nothing | EOF → reconnect loop; reads fall back? | same |

### C. A whole DC fails

| id | incident | cluster | expected client | watch |
|---|---|---|---|---|
| C1 | isolate `dc1`, 60s | shard 1 fails over; 2 replicas lost | as A1 | same |
| C2 | isolate `dc1` and `dc2` (two DCs down) | dc3 alone: 1 master of 3, cluster_state fail, CLUSTERDOWN everywhere | CLUSTERDOWN → sleep ≥1.28s → retry; overall timeout 3s | error kinds; whether shard 3 (its master in dc3) still serves anything |
| C3 | C2 with `cluster-require-full-coverage no` | dc3-s3 keeps serving its slots | shards 1,2 fail (CLUSTERDOWN / timeouts), shard 3 ok | per-shard split |
| C4 | client and `dc1` isolated together (client in the minority DC) | shard 1 master in dc1 loses quorum → CLUSTERDOWN after 5s; others fail over | all three shards fail for the client; after heal it must find the masters outside dc1 | recovery after heal |
| C5 | rolling DC loss: isolate dc1 20s, heal, 15s later dc2, heal, dc3, heal | masters chase each other around | three failovers | masters end up where; whether the client follows each |
| C6 | flapping dc1: 3s on / 3s off ×8 (below node-timeout) | PFAIL churn, no failover | timeouts in bursts | error count; any refresh storms |
| C7 | flapping dc1: 8s on / 8s off ×4 (above node-timeout) | failover each time; old master comes back and gets re-promoted? | | number of failovers, lost writes |

### D. Partitions and asymmetry

| id | incident | cluster | expected client | watch |
|---|---|---|---|---|
| D1 | cut `client` ↔ `dc1` only | healthy, no failover | shard 1 times out for the whole incident, nothing the cluster can tell the client | throughput collapse from head-of-line blocking |
| D2 | cut-oneway `dc1-s1` → everyone (its packets out are dropped) | failover | timeouts (requests arrive, replies don't) | same as A1 |
| D3 | cut-oneway everyone → `dc1-s1` (packets in are dropped) | failover | timeouts | same |
| D4 | **split-brain write window**: cut `dc1-s1` from all other nodes, keep client ↔ `dc1-s1` | old master accepts writes until it notices lost quorum (~5s), replica promoted at ~6s, old master demoted on heal | writes acked by the old master in the window are LOST; after CLUSTERDOWN the client sleeps and retries on the same stale node | LOST count; time to first CLUSTERDOWN; client stuck until heal |
| D5 | cut client ↔ the three seed nodes (masters) | healthy | all shards time out; no MOVED will ever arrive | after `rebalance` moves masters to the replicas: does the client notice? (no error ever triggers a refresh) |
| D6 | cut `dc1` ↔ `dc2`, then `rebalance` | with dc3 seeing both, no failover | nothing | the manual failover still works through dc3 |

### E. Network quality

| id | incident | cluster | expected client | watch |
|---|---|---|---|---|
| E1 | 100ms delay into every node | nothing | p50 ~100ms; Nagle doubles it without `--nodelay` | with/without `--nodelay` |
| E2 | 900ms delay into `dc1-s1` (just under response timeout) | nothing (node-timeout 5s) | p99 near 1s, some timeouts, retries that also time out | retry amplification: ops sent vs ops acked |
| E3 | 1500ms delay into `dc1-s1` (over response timeout) | nothing | shard 1 100% timeouts; healthy cluster that the client cannot use | same as D1 |
| E4 | 5% loss into `dc1-s1` | nothing | retransmits, p99 ~200ms+ | |
| E5 | 25% loss into `dc1-s1` | PFAIL flaps possible | timeouts | failover or not |
| E6 | 60% loss into `dc1-s1` | failover likely | timeouts then recovery? | whether the client finds the new master |
| E7 | 100ms delay into `client` (slow client link) | nothing | p50 100ms everywhere | base for comparing with E1 |

### F. Cluster operations

| id | incident | cluster | expected client | watch |
|---|---|---|---|---|
| F1 | reshard 2000 slots s1 → s2 under load, then back | ASK during migration, MOVED after | ASK redirect per request, refresh on MOVED | error count, latency, whether reads/writes hit TRYAGAIN |
| F2 | add node `dc1-s4` as a new master and rebalance slots to it | new node in the slot map | MOVED to an unknown address → connect on demand | errors, refresh count |
| F3 | move slots off `dc1-s4` and `del-node` it | node gone | its connection removed on refresh | stale connection errors |
| F4 | restart all 9 nodes at once (full cluster restart) | cluster back in ~10s | every connection drops → reconnect from seeds | time to recover; errors |
| F5 | rolling restart: every node, one at a time, 10s apart | three failovers (masters), replicas just rejoin | MOVED each time | total errors, final layout |
| F6 | server `CONFIG SET timeout 3` with 2 ops/s load (idle connections dropped by the server) | nothing | EOF on next use → reconnect+retry, ideally 0 errors | error count |
| F7 | `CLIENT KILL TYPE normal` on all masters, under load | nothing | reset → reconnect+retry | error count |

### G. Client knobs, same incident (A1-style blackhole of `dc1-s1`)

| id | knob | expectation | watch |
|---|---|---|---|
| G1 | library defaults: 16 retries, overall timeout = response timeout | each request on shard 1 burns 1s then fails; retries never get time | compare with G2 |
| G2 | 16 retries, overall timeout none | a request to shard 1 hangs 16s+ | worst-case latency |
| G3 | `--read-replicas` | reads of shard 1 keep working from replicas, writes stuck | read/write split |
| G4 | open-loop load (no head-of-line blocking) | shards 2,3 unaffected, shard 1 100% errors | true availability figure |
| G5 | `--concurrency-limit 8` | backlog bounded | errors vs latency |
| G6 | `--tcp-user-timeout 2000` | kernel aborts the blackholed connection after 2s → EOF → reconnect → fallback → MOVED → recovers without heal | time to recover |
| G7 | `--keepalive 1s` only | keepalive does not fire with data in flight; no change from A1 | confirm |

### H. Multi-key, pipelines, transactions (under A3: kill master)

| id | workload | expectation | watch |
|---|---|---|---|
| H1 | MULTI/EXEC of 3 SETs on one hash tag | atomic; EXEC during failover fails with EXECABORT/MOVED | partial writes? |
| H2 | pipeline of 10 SETs on one hash tag | retried as a unit | errors |
| H3 | MGET/MSET across slots (fan-out) | fan-out errors are not retried | errors |

### I. Connection establishment

| id | setup | expectation | watch |
|---|---|---|---|
| I1 | connect with 1 of 3 seeds blackholed | connects after the 1s connect timeout (waits for all seeds) | connect latency |
| I2 | connect with 2 of 3 seeds killed (refused) | immediate | |
| I3 | connect with all 3 seeds down but cluster alive | fails | error text |
| I4 | connect while cluster_state is fail (C2) | CLUSTER SLOTS still answers; connection works, commands get CLUSTERDOWN | |
| I5 | 50 new connections per second during A3 | connect storms | connect failures |

### J. App-side self-healing (`--self-heal poke|rebuild|both`, no library change)

Reruns of the incidents the library never recovers from, with the client's
`Healer` active: after 3 consecutive failures on a shard, poke a replica
(its MOVED triggers the library's own slot refresh) and/or rebuild the
connection from the seeds. Added after the A–I phase.

| id | setup | expectation | watch |
|---|---|---|---|
| J1 | A1 blackhole, poke | refresh after each poke; recovers at failover | errors, recovery time |
| J2 | A1, rebuild | new connection sees the new master | rebuild time |
| J3 | A1, both | poke suffices | |
| J4 | A4 SIGSTOP, both | the case TCP_USER_TIMEOUT cannot fix | |
| J5 | A8 CLIENT PAUSE WRITE, both | no failover, nothing to discover | unchanged |
| J6 | D5 client cut from seeds (9 seeds), both | poke works through existing replica connections | |
| J7 | H1 MULTI/EXEC, both | EXECABORT(MOVED) counted as a strike; rebuild fixes the stale map | |
| J8 | A1 + TCP_USER_TIMEOUT 2s, both | do the two stack? | |
| J9 | A3 kill -9, both | regression check | not worse than A3 |

## Order

A, D4, C, D, E, F, G, H, I, B. A1/A3 first because most later rows compare
with them.
