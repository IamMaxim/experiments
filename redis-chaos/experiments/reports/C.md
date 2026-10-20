# C. A whole DC fails

All runs: 400 ops/s SET, 16 closed-loop workers, 3s overall timeout.

| run | incident | errors | lost | shards hit | client recovered |
|---|---|---|---|---|---|
| C1 `C1-20261020-171137` | isolate dc1, 30s | 240 timeouts | 0 | s1 | after heal + TCP backoff |
| C2 `C2-20261020-171242` | isolate dc1 and dc2 | 240 timeouts | **21** | all | t=47 (12s after heal) |
| C3 `C3-20261020-171410` | C2 with `cluster-require-full-coverage no` | 240 timeouts | **20** | all | ~42 |
| C4 `C4-20261020-171534` | client + dc1 isolated together | 161 timeouts | 0 | all | t=39 |
| C5 `C5-20261020-171628` | dc1, then dc2, then dc3 | 409 timeouts | 0 | all | each time after heal |
| C6 `C6-20261020-171830` | dc1 flapping 3s/3s ×8 | 301 timeouts | 0 | s1, 33 of 48s | after the flapping stopped |
| C7 `C7-20261020-171945` | dc1 flapping 8s/8s ×4 | 64 timeouts | 0 | s1, 9s | yes, at the first heal |

## C1: one DC

Same as A1 (its master is the only thing the client needs from dc1): the
two replicas in dc1 are irrelevant to a master-only client. Failover at ~15s,
client stuck until the heal, then the TCP backoff.

## C2: two DCs, no quorum

Each DC alone holds one master. With dc1 and dc2 isolated, no partition has
two masters, so nothing can be elected, and every node goes
`cluster_state fail` at 13.6s. From the client (which can only reach dc3):

- shards 1 and 2 (masters in dc1/dc2): unreachable → timeouts from 9s;
- shard 3 (master dc3-s3, reachable): **writes accepted until 18s**, then
  CLUSTERDOWN (hidden behind the 3s overall timeout as `timed out`).

After the heal at 35s the cluster did not simply come back. dc1-s3 (a
replica of shard 3 sitting in dc1) had been trying to get elected the whole
time; when the links came back the masters still had dc3-s3 flagged FAIL
and granted the vote. dc1-s3 became master at epoch 43 and **dc3-s3, alive
the whole time, was demoted and discarded its data**. The writes it had
acknowledged between 5s and 18s (21 keys in this run, 20 in C3) were lost.
Two more failovers followed (epochs 44, 45; shards 1 and 2 ended up on
dc2-s3 and dc3-s2) and the cluster was only stable at ~50s.

The client refreshed once, at 47s, after the first MOVED got through.

This is cluster behaviour, not client behaviour, but it matters for the
client: after a cluster-wide incident the topology can change *after* the
network is back, so a single refresh at heal time is not enough.

## C3: two DCs, `cluster-require-full-coverage no`

Expected shard 3 to keep serving. It did not: a master that has lost the
majority of masters for node-timeout stops serving regardless of the
coverage setting (`CLUSTERDOWN`), so the outcome matched C2 (240 timeouts,
20 lost after the same post-heal election).

## C4: client in the minority DC

The client sees only dc1: shards 2 and 3 unreachable from 9s; shard 1 served
by dc1-s1 until it loses quorum (~13s), then CLUSTERDOWN-as-timeout from
18s. The majority elected a new shard-1 master in dc2/dc3 at ~16s, so after
the heal the client had three stale connections, two of them dead sockets
(dc2-s2, dc3-s3) and one demoted node (dc1-s1). Recovery at ~39s: the
demoted node answered MOVED quickly; the two dead sockets revived after
their backoff (30s outage, ~4s wait).

## C5: rolling DC loss

The runs before this one had already moved shard-1's master to dc2 (the old
`rebalance` put masters back but not replica attachments; fixed since). The
sequence then showed the real hazard: after the first failover two masters
lived in dc2, so isolating dc2 at 40s removed the quorum and **every shard
was down** from 47s to 63s, not just the ones in dc2. The client saw it as
timeouts on all shards. Each heal was followed by the same TCP backoff wait
(s1 recovered at 31.6s after the 25s heal; s3 at 104.4s after the 95s heal).

Operationally: after a DC failover, masters must be moved back before the
next DC fails, or the cluster's quorum depends on one DC.

## C6: flapping below node-timeout

The cluster never noticed (no PFAIL reached FAIL, no failover, no epoch
change). The client lost shard 1 for 33 of the 48s. Each 3s blackhole hits
the socket in the middle of a request, the retransmit timer doubles, and the
next window of connectivity is often shorter than the current RTO, so the
data is not retransmitted while the link is up. No library log line at all:
nothing happened that redis-rs reacts to.

## C7: flapping above node-timeout

The first outage produced a failover (dc3-s2, by then a replica of shard 1,
took it); when dc1 came back, dc1-s1 answered MOVED, the client refreshed
(18.2s) and followed the new master, which was never in dc1 again. 64
errors, all in the first window. Three more flaps of dc1 were invisible to
the client. In the cluster, however, replicas migrated between shards with
each flap and the final layout had shard 1's master in dc3 and three
replicas on shard 2; the old `rebalance` could not restore it.

## Implications for redis-rs

- Everything from A1 (timeouts → reconnect + refresh).
- A refresh at the moment of heal is not the end of the story (C2): keep
  refreshing while MOVED/CLUSTERDOWN keep arriving, with rate limiting.
- Flapping (C6) is the strongest case for reconnecting after a timeout:
  a fresh SYN gets through on the next good window while the old socket's
  retransmit timer sleeps through it.
