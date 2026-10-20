# J. App-side self-healing (no library change)

The client gained a `Healer` (`client/src/main.rs`): workers take the
current `ClusterConnection` handle per operation and report every result.
After `--heal-after` (3) consecutive heal-worthy errors on one shard
(timeouts, I/O errors, MOVED/ASK/CLUSTERDOWN/TRYAGAIN, or an `EXECABORT`
carrying MOVED) it runs one of:

- **poke**: `SET` a probe key on the shard, routed `ByAddress` to a replica
  of that shard (addresses from `CLUSTER SLOTS` at startup). The replica
  answers MOVED; redis-rs follows the redirect and schedules `refresh_slots`
  (`PollFlushAction::RebuildSlots` fires on receipt of the MOVED, before the
  redirected attempt is even sent). The probe's own result is ignored.
- **rebuild**: `client.get_async_connection()` again and swap the handle
  (`RwLock<ClusterConnection>`); the new connection starts from a fresh
  `CLUSTER SLOTS`.
- **both**: poke, then rebuild if the shard is still failing one
  `--heal-interval-ms` (3s) later.

Heals are serialised and rate-limited to one per interval. Baselines are the
runs from groups A, D, G and H with the same incident.

| run | incident | mode | errors | shard outage | baseline |
|---|---|---|---|---|---|
| J1 `J1-20261020-203105` | A1 blackhole dc1-s1 60s | poke | **25** | 9–15s | A1: 400, 9–83s |
| J2 `J2-20261020-203230` | A1 | rebuild | **35** | 9–15s | |
| J3 `J3-20261020-203357` | A1 | both | **27** | 9–15s | |
| J4 `J4-20261020-203524` | A4 SIGSTOP dc3-s3 | both | **22** | 9–14s | A4: 144, 9–33s |
| J5 `J5-20261020-203620` | A8 CLIENT PAUSE WRITE | both | 140 | 9–34s | A8: 144, 9–34s |
| J6 `J6-20261020-203714` | D5 client cut from seeds, masters moved (9 seeds) | both | **48** | 9–17s | D5: 272, 9–57s |
| J7 `J7-20261020-203942` | H1 MULTI/EXEC, kill dc2-s2 | both | **888** | 6–15s | H1: 5745, never |
| J8 `J8-20261020-204036` | A1 + `TCP_USER_TIMEOUT 2s` | both | 33 | 8–16s | G6: 22, 8–13s |
| J9 `J9-20261020-204200` | A3 kill -9 dc2-s2 | both | 25 | 9–15s | A3: 32, 9–16s |

Every run: 0 lost writes, and recovery before the heal in all but J5.

## How each one recovered

**J1 (poke).** Three timeouts at 8.0s trigger the first poke. The replica's
MOVED starts a refresh immediately; the refresh dials dead dc1-s1 under the
write lock (finding 4), so every shard stalls ~1s per refresh and the MOVED
→ refresh → stale-map loop runs once per second (the same storm as A3).
When the cluster finishes the failover at ~13s, the next refresh picks up
dc2-s1 as master and shard 1 is back at 14.0s. The poke itself returned
`timed out` after 3s: its redirect went to the dead master. Harmless.

**J2 (rebuild).** First rebuild at 8.0s took 2.0s (dead seed + dead node in
the map, as in I1) and produced the same stale map; three more timeouts,
second rebuild at 13.0s took 1.0s and found the new master; shard 1 back at
14.0s. Each rebuild opens 9 new connections; the old handle is dropped once
the last worker finishes with it.

**J3 (both)** behaved like J1; the rebuild branch never ran because the poke
had already fixed it.

**J4 (SIGSTOP).** A4 could not be helped by `TCP_USER_TIMEOUT` (the kernel
keeps ACKing for a stopped process). The poke path does not care: 22
errors, outage 9–14s, against 144 and 9–33s.

**J5 (CLIENT PAUSE WRITE).** No failover happens (the bus is alive), so no
map change exists to discover. Pokes and a rebuild ran and changed nothing.
Same as A8. Only a failover (or `--read-replicas` for reads) helps here.

**J6 (D5).** The client is cut from the three seed masters at 5s; the
operator fails the masters over to the replicas at ~15s. D5 took until 57s
(heal + TCP backoff). Here the poke's MOVED-triggered refresh ran each
second until the failover landed; all three shards back at 17.0s. Note this
run used all nine nodes as seeds so a rebuild could have worked too; with
the three default seeds a rebuild would fail (I3) while the poke still
works, because the poke goes through the existing map's replica
connections.

**J7 (transactions).** H1 never recovered. Here the `EXECABORT` payload is
scanned for `Moved`; three of them at 5.0s trigger a poke. The poke's
refresh ran before the failover completed, so it produced the same stale
map; the `both` fallback rebuilt the connection at 14.3s (1.0s) and shard 2
was back at 16.0s. 888 errors instead of 5745, outage 10s instead of the
whole run. 703 of the errors are shard 2 during the 10s; the rest are the
election-window CLUSTERDOWN inside `EXECABORT` on the other shards, as in
H1.

**J8 (+TCP_USER_TIMEOUT).** No better than G6 alone (33 vs 22 errors,
outage 8–16s vs 8–13s). The kernel error already starts the library's own
reconnect-and-refresh path at 7.3s; the pokes on top added a few more
refreshes to the storm. The 15 surfaced MOVED errors are finding 5, same as
G6. The two mechanisms do not stack; TCP_USER_TIMEOUT wins when it applies,
the healer covers the cases it cannot (J4).

**J9 (kill -9).** The regression check: 25 vs 32 errors, same window. The
pokes added nothing and cost nothing.

## Reading

Poke is the better primitive: it costs one request, works through the map
the library already has, and needs no reachable seed. Its limits are the
library's: the refresh it triggers stalls everything for the connect
timeout of each dead node (finding 4), and it has no effect until the
cluster has actually elected a new master, so recovery lands at
failover + 1–2s, which is the best any client can do. Rebuild is the
fallback for the cases where the library's own refresh path cannot help
(H1-type NoRetry errors with a stale map, or a map so stale that no replica
address in it is reachable), at the cost of 9 fresh connections and 1–2s
per unreachable node.

What it does not fix: anything without a failover (J5), the lock stall
(the refresh storm is visible as p99 ≈ 1–2s on the healthy shards in every
J run), and the MOVED/broken-pipe leaks (findings 5, 7), which are per
request and happen before the healer can act.

## Cost of the quick & dirty version

~150 lines in the application: a wrapper that hands out the handle, a
per-shard strike counter, `CLUSTER SLOTS` parsing for replica addresses,
and the two actions. Pubsub subscriptions would not survive a rebuild; the
poke writes a key (`rcc:heal:{tag}`) into the cluster. Both actions are
bounded by the request's `overall_response_timeout`, so a heal takes at
most 3s per replica tried.
