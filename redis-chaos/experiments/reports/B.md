# B. One replica fails

Mixed load (50% GET, 50% SET), replica dc2-s1 of shard 1.

| run | incident | reads from | errors | stale reads | notes |
|---|---|---|---|---|---|
| B1 `B1-20261020-170849` | blackhole dc2-s1 | masters | **0** | 0 | 20016 ok, max 5.7ms |
| B2 `B2-20261020-170943` | blackhole dc2-s1 | replicas (`RandomReplicaStrategy`) | 85 timeouts, p99 2s for the whole incident **and 17s past the heal** | 20 | throughput 5723 vs 20016 |
| B3 `B3-20261020-171043` | kill dc2-s1 | replicas | **0** | 63 | 20016 ok |

## B1: master-only client

Nothing. The cluster flagged dc2-s1 FAIL at ~13s; the client has a connection
to it (it connects to every node in `CLUSTER SLOTS`) but never sends anything
on it. The slot refresh after the heal re-checked it and moved on.

## B2: reads from replicas, replica blackholed

Half the shard-1 reads pick dc2-s1 (two replicas, random), hang 1s, retry
(`RetryImmediately`, so the same random pick), and either land on dc3-s1 the
second time (p99 2040ms) or time out after 3s. The slot map was never
refreshed. Checked afterwards: once the cluster flags a replica FAIL (~13s),
`CLUSTER SLOTS` stops listing it, so a single refresh would have removed
dc2-s1 from the read candidates for the rest of the incident. Nothing
triggered one.

After the heal the connection to dc2-s1 stayed dead for the TCP backoff
(A1 mechanism), so the errors continued to the end of the run at 52s.

The 20 stale reads are expected with async replication (a replica a few ms
behind). The run also reported 16 "lost" writes, which were stale reads
during the verification pass, because the verification used the same
replica-reading connection while dc2-s1 was resyncing; the client now
verifies through a master-only connection.

## B3: reads from replicas, replica killed

Connection refused → `Reconnect(dc2-s1)` → while it is `Reconnecting` the
shard fallback picks another node (dc3-s1 or the master), so reads carried on
with no errors. 63 stale reads over 30s (replica lag), as expected. The
reconnect loop retried dc2-s1 with 1s connect timeouts in the background; no
effect on the foreground.

## Reading

Same split as A1 vs A3: a dead socket is handled, a silent socket is not.
With read routing enabled the problem is bigger because a read strategy picks
from a list that includes nodes the client has every reason to believe are
dead.

## Implications for redis-rs

- Read strategies need health input: exclude nodes with recent consecutive
  timeouts (or in `Reconnecting`) from `ReadCandidates`, and fall back to the
  master.
- Refresh on timeouts (finding 1): `CLUSTER SLOTS` already omits FAIL
  replicas, so the cluster's own health view would have fixed this at ~13s.
- Same timeout-driven reconnect as A1.
