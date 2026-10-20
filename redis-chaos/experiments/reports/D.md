# D. Partitions and asymmetry

D4 (split brain) has its own report. All runs 400 ops/s SET, 16 workers.

| run | incident | cluster | errors | lost | client recovered |
|---|---|---|---|---|---|
| D1 `D1-20261020-172313` | client ↔ dc1 cut | nothing | 240 timeouts on s1 | 0, **240 "ahead"** | after heal + TCP backoff (still failing at 52s, heal at 35s) |
| D2 `D2-20261020-172416` | dc1-s1's outbound packets dropped | failover | 240 timeouts | 0 | after heal + backoff |
| D3 `D3-20261020-172528` | dc1-s1's inbound packets dropped | failover | 240 timeouts | 0 | after heal + backoff |
| D5 `D5-20261020-172632` | client ↔ 3 seed masters cut; masters then moved to the replicas | 3 manual failovers | 272 timeouts on all shards | 0 | 57.4s (heal 45s) |
| D6 `D6-20261020-172737` | dc1 ↔ dc2 cut, `CLUSTER FAILOVER` on dc2-s1 | manual failover through dc3 | **0** | 0 | n/a |

## D1: only the client loses dc1

The cluster is healthy and the client times out on shard 1 for the whole
incident. There is nothing to refresh: a correct refresh would say
"dc1-s1 is the master", which is true. Only a client-side policy helps
(fail fast after N timeouts, or serve reads from the replicas in dc2/dc3,
which were reachable the whole time).

Verification found **240 writes "ahead"**: every write that the client
reported as timed out was executed later. The requests sat in the socket's
send queue and were delivered by TCP retransmission after the heal, up to
tens of seconds later, and the values they wrote overtook the ones the
client had acknowledged since. A timeout in redis-rs means "no answer yet",
never "not executed"; with a retrying client each timed-out SET may also run
up to four times.

## D2 / D3: one-way cuts

Both lead to a failover (a node that cannot send PONGs, or cannot receive
PINGs, gets flagged FAIL either way), and both look to the client exactly
like A1: requests reach the node but the reply does not come back (D2), or
never arrive (D3). Timeouts, no refresh, stale socket after the heal.

## D5: the seeds are unreachable, the masters move elsewhere

From 5s the client cannot reach dc1-s1, dc2-s2, dc3-s3 (every shard's
master). At 15s the masters are failed over manually to dc2-s1, dc3-s2,
dc1-s3, all reachable by the client. **The client never noticed**: no error
it receives (timeouts only) triggers a refresh, so for 42s it kept sending
to three demoted, unreachable nodes while a fully reachable cluster served
the same slots. First refresh at 57.4s, 12s after the heal, when the old
masters' sockets came back and answered MOVED.

This is the cleanest demonstration that the library needs a timeout-driven
or periodic topology check: the information was one `CLUSTER SLOTS` to any
of the six reachable nodes away.

## D6: manual failover across a partition

`CLUSTER FAILOVER` on dc2-s1 while dc1 (its master) was unreachable from
dc2: Redis still completed it (dc3 relays the bus traffic; the manual
failover waits for the replication offset then runs an election, which
needs 2 of 3 masters, dc2-s2 and dc3-s3). The client got MOVED from the
demoted master, refreshed once, 0 errors.

## Implications for redis-rs

- D5: a topology check that does not depend on receiving a redirect.
- D1: expose "N consecutive timeouts on node X" to the application or act
  on it (fail fast / read from replica); document that a timed-out write
  can still execute later.
