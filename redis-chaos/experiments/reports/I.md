# I. Connection establishment

`rcc connect-probe`: build a `ClusterClient` and open an async connection
once per second (I5: 50 at once), report the time. Seeds are the three
designed masters; connect timeout 1s.

| run | incident | before | during | after heal |
|---|---|---|---|---|
| I1 `I1-20261020-175425` | seed dc1-s1 blackholed | 1–2ms | **1.0s ×11, 2.0s ×4** | 1–2ms (one 1s) |
| I2 `I2-20261020-175509` | seeds dc1-s1, dc2-s2 ICMP-rejected | 2ms | **2.0s ×8**, 1s ×1, fast ×2 | fast (one 2s) |
| I3 `I3-20261020-175557` | no seed reachable (cluster healthy) | 2ms | **FAILED after 1.0s** ×18: `Failed to create initial connections - Io: timed out` | fast (one failure) |
| I4 `I4-20261020-175643` | cluster_state fail (dc1, dc2 isolated) | 1–2ms | **ok in 2.0s** ×11 | fast (one 2s) |
| I5 `I5-20261020-175802` | 50/s while dc2-s2 is killed | 31–53ms | 1s ×500, 2s ×200, fast ×50 | 1s ×50, then fast |

## What the times are made of

`get_async_connection` does `create_initial_connections` (all seeds in
parallel, waits for all of them, so one dead seed costs one connect
timeout), then `refresh_slots`, which runs `CLUSTER SLOTS` on a seed and
then **connects to every node in the map** before returning. A dead node in
the map costs another connect timeout. Hence:

- I1: 1s (dead seed) + sometimes 1s more (the same dead node again during
  the refresh's connection pass, when the first attempt raced).
- I2: the ICMP reject fails the first SYN instantly (`Host is unreachable`),
  but the retry inside `connect_and_check`/the refresh pass then waits the
  full timeout; 2s in most cases.
- I4: the seeds answer, `CLUSTER SLOTS` answers (Redis serves it even with
  `cluster_state fail`), and the refresh then dials the six nodes in dc1/dc2
  for 1s, twice. The connection works; commands on it then get CLUSTERDOWN.
- I5: 50 parallel connects per second all paid the dead node's timeout; no
  failures, no refused connections, nothing on the surviving nodes
  complained (each `rcc` instance holds 9 connections, so ~450 new
  connections per second landed on the cluster during the probe).
- I3: fails after the connect timeout with a generic message that does not
  say which seeds were tried.

## Reading

Connecting is robust (no failures unless every seed is down) but slow by
one or two connect timeouts whenever any node of the cluster, seed or not,
is unreachable. The slot-refresh connection pass is the same code that
stalls live traffic in A3; here it stalls new clients.

## Implications for redis-rs

- Return the connection as soon as one seed answered and the slot map is
  known; connect to the other nodes lazily or in the background.
- Do not block the initial connection on nodes that fail to connect; mark
  them `Reconnecting` and move on.
- Include the per-seed errors in the "Failed to create initial connections"
  message.
