# Experiment summary

55 runs on 2026-10-20 against Redis 8.2.2 cluster (3 DCs × 3 shards,
node-timeout 5s) with redis-rs at 14e710d (async cluster client). Plan in
`PLAN.md`, one report per group in `reports/`, raw runs in `runs/`.

## Verdict

The cluster side did what Redis Cluster promises in every run: failover in
7–15s, no acknowledged write lost except in the two cases where the design
loses them (a minority master accepting writes before it notices, D4; a
stale replica elected after a partition heals, C2/C3). Resharding, scaling
and manual failovers were invisible to the client.

The client side has one dominant problem and a handful of smaller ones:

**redis-rs only learns about topology changes from a reachable node that
says MOVED.** Anything that makes the old master *unreachable but not
closed* (blackhole, ICMP reject, SIGSTOP, one-way cut, heavy loss, long
delay, a client-side partition) leaves the client sending to the stale node
until the heal, and then until the stale socket's TCP retransmit timer
fires (up to ~45s more after a 60s outage). The cluster had a new master
within 15s in all of these; the client used it in none of them.

## Findings, ranked by impact

| # | finding | seen in | fix direction |
|---|---|---|---|
| 1 | Timeouts never trigger a reconnect or a slot refresh; the request is retried on the same node until retries or the overall budget run out | A1 A2 A4 B2 C1 C4–C6 D1–D3 D5 E3 E6 G1 G2 G7 | count consecutive timeouts per node; after N, reconnect and refresh from another node; add a periodic/background topology check |
| 2 | A blackholed socket stays "connected" through TCP backoff; recovery after the heal waits for the retransmit timer (24s after a 30s outage, 46s after 60s) | A1 A2 C1 C6 D1–D3 D5 | set `TCP_USER_TIMEOUT` by default (G6: outage 30s → 5s, no heal needed) or drop a connection after N timeouts |
| 3 | **MULTI/EXEC never recovers from a failover**: `EXECABORT` wrapping MOVED is `NoRetry`, no redirect, no refresh; shard stayed dead through failover and heal | H1 | inspect the errors inside `EXECABORT`; redirect/refresh/retry like a single command |
| 4 | `refresh_slots` holds the connection-map write lock while dialing every node, including dead ones (1s connect timeout each); every request on every shard stalls for the duration, and a MOVED → refresh → stale map loop repeats it once per second until the cluster finishes the failover | A3 A5 F4 F5 G6 I1–I5 | refresh the map, swap it in, repair connections outside the lock; skip nodes in `Reconnecting`; coalesce/rate-limit refreshes |
| 5 | MOVED (and `Connection refused`, `broken pipe`) returned to the application after retries are exhausted by a redirect to a node that is down; retries are shared between "node dead" and "follow redirect" | A5 F4 F5 F6 G6 | a redirect to an unreachable address → refresh from elsewhere; never surface a redirect as the final error |
| 6 | `CLUSTERDOWN`/`TRYAGAIN` → sleep ≥1.28s then retry **on the same node**; the overall timeout then reports the failure as `timed out`, hiding the server error | A6 D4 F5 C4 | refresh from another node on CLUSTERDOWN; first backoff in the tens of ms; surface the last server error |
| 7 | A failed slot refresh fails every in-flight request with the refresh's I/O error (`broken pipe` on a server-closed idle connection), consuming retries | F6 (8 of 51 requests) | do not use a connection that just errored for the refresh; keep requests pending while the refresh is retried |
| 8 | Read strategies keep routing to a replica that times out; a refresh would have dropped it (`CLUSTER SLOTS` omits FAIL replicas) but nothing triggers one | B2 | exclude nodes with recent timeouts / in `Reconnecting`; refresh on timeouts |
| 9 | `TCP_NODELAY` off by default doubles latency on any real network (200ms vs 100ms at 100ms RTT) | E1b | default on |
| 10 | Connecting waits for every seed and then for every node in the map: +1–2s per unreachable node; `cluster_state fail` connects fine but says nothing | I1 I2 I4 I5 | return after the first seed and `CLUSTER SLOTS`; connect the rest lazily |
| 11 | `overall_response_timeout` defaults to `response_timeout`, so retries are dead code with defaults; with no overall timeout a request hangs 17s (16 × 1s) | G1 G2 | coherent defaults (e.g. overall = 3 × response) |
| 12 | `BUSY` is an unknown extension error, fails fast; defensible but undocumented | A9 | add to known errors, decide policy |
| 13 | Timed-out writes execute later (TCP delivers them after the heal): "timed out" is not "not executed", and a retried write may run several times | D1 (240 of 240) | document; make retries of non-idempotent commands opt-in |

## App-side workaround (group J, `reports/J.md`)

Without touching redis-rs, the application can recover from findings 1–3
by counting consecutive failures per shard and, after three, sending a
write to a replica of that shard by address: the replica's MOVED makes the
library refresh its slot map. A connection rebuild from the seeds is the
fallback for transactions, whose `EXECABORT` never triggers a refresh.

| incident | library alone | with `--self-heal both` |
|---|---|---|
| A1 blackhole 60s | 400 errors, shard out 9–83s | 25–35 errors, 9–15s |
| A4 SIGSTOP | 144, 9–33s | 22, 9–14s |
| D5 client cut from masters, masters moved | 272, 9–57s | 48, 9–17s |
| H1 MULTI/EXEC + kill | 5745, never | 888, 6–15s |
| A3 kill -9 (regression) | 32, 9–16s | 25, 9–15s |
| A8 CLIENT PAUSE WRITE (no failover) | 144 | 140, unchanged |

Recovery lands 1–2s after the cluster's failover in every case, which is
the floor. Findings 4–7 (lock stall, surfaced MOVED, CLUSTERDOWN backoff,
failed-refresh errors) remain and need library changes.

## What worked without a scratch

Manual failover (A7), resharding (F1), add/remove node (F2), `CLIENT KILL`
(F7), losing a replica (B1, B3), restart within node-timeout (A6, 16
errors), partition that keeps a majority (D6), ≤25% loss (E4, E5), latency
under the response timeout (E2).

## Cluster-side observations worth knowing

- **Split brain window** (D4): a minority master acknowledges writes for
  ~8s before `cluster-node-timeout` expires. ~900 of 10 000 writes lost.
  Only `WAIT` on the client can shrink it.
- **Post-partition stale election** (C2, C3): after two DCs come back, a
  replica that had been requesting votes gets them before the old FAIL
  flags clear, demoting a live master and losing its recent writes.
  `cluster-require-full-coverage no` changes nothing here.
- **Masters pile up** (C5, C7): after one DC failover, two masters share a
  DC; the next DC loss removes the quorum and takes every shard down.
  Replica migration then reshuffles replicas across shards. `chaos
  rebalance` now restores the full layout; in production that is an
  operator's job after every failover.
- A busy Lua script (A9) and `CLIENT PAUSE WRITE` (A8) make a master
  unusable for clients while the cluster keeps it healthy; only the client
  can notice.
- 60% packet loss (E6) never reached FAIL in 40s; the client was dead on
  that shard the whole time.

## Harness notes

- 16 closed-loop workers: one dead shard stalls all 16 (each waits 3s per
  shard-1 key). Open-loop runs (G4) show the true picture: the other two
  shards unaffected. Both patterns exist in real applications.
- The per-second lines, the library's log and the cluster snapshots share
  one clock; `bin/digest <run>` condenses a run.
