# E. Network quality

400 ops/s SET, 16 closed-loop workers, 1s response / 3s overall timeout.

| run | netem | cluster | ok/s during | p50 | p99 | errors |
|---|---|---|---|---|---|---|
| E1a `E1a-20261020-172833` | 100ms into every node, `--nodelay` | nothing | 160 | 100.2ms | 102ms | 0 |
| E1b `E1b-20261020-172920` | same, Nagle on (library default) | nothing | 91–99 | **200ms** | 201ms | 0 |
| E7 `E7-20261020-173459` | 100ms into the client | nothing | 160 | 100.3ms | 100.9ms | 0 |
| E2 `E2-20261020-173007` | 900ms into dc1-s1 | nothing | 40–64 | 0.3ms | 900ms | 0, max 2.5s |
| E3 `E3-20261020-173104` | 1500ms into dc1-s1 | nothing (bus RTT 1.5s < node-timeout) | ~30 | | | 145 timeouts, s1 dead 30s |
| E4 `E4-20261020-173201` | 5% loss into dc1-s1 | nothing | 346–400 | 0.45ms | 200–240ms | 0 |
| E5 `E5-20261020-173258` | 25% loss into dc1-s1 | nothing | 145–275 | 0.45ms | 250–610ms | 0, max 1.4s |
| E6 `E6-20261020-173353` | 60% loss into dc1-s1 | PFAIL flickers, **no failover** | ~30 | | | 188 timeouts, s1 dead 40s |

`ok/s during` is throttled by the closed-loop harness (16 workers × 1 request
in flight); it measures latency more than capacity.

## E1a / E1b / E7: Nagle

With `TCP_NODELAY` off, a request that fits in one segment waits for the ACK
of the previous one before leaving the host (Nagle + delayed ACK), so every
command costs two round trips: 200ms instead of 100ms, half the throughput.
redis-rs leaves Nagle on by default (`TcpSettings::default()`); every other
Redis client turns it off. E7 shows the same 100ms whether the delay is at the
client or at the nodes.

## E2: just under the response timeout

No errors, but each shard-1 request takes 900ms and the first retry after a
rare timeout adds another 900ms (max 2.5s). Throughput falls to 40–64
ops/s because the workers spend their time waiting. No cluster reaction, no
library reaction; the right behaviour.

## E3: over the response timeout

A healthy cluster (bus RTT 1.5s, node-timeout 5s) the client cannot use:
every shard-1 request times out at 1s, is retried on the same node, and fails
after the 3s overall budget. 145 errors in 30s (the harness caps the error
rate). On heal, the backlog of retransmitted requests drained with a 2.9s
max latency and one more timeout.

Nothing to refresh here either; what is missing is a per-node signal
("dc1-s1 answers in 1.5s, stop retrying it immediately").

## E4 / E5: loss

TCP absorbs 5% loss with a 200ms p99 (one retransmit timeout) and no errors.
At 25% the p99 reaches 600ms and throughput halves, still no errors, no
cluster reaction (the bus tolerates it).

## E6: 60% loss

The cluster bus still kept dc1-s1 alive (PFAIL came and went; no FAIL, no
election in 40s). The client could not get a request and reply through
within 1s most of the time: 188 timeouts, shard 1 effectively dead for the
whole 40s. After the heal the socket's retransmit backoff delayed recovery
to ~48s.

A gray failure: the cluster says healthy, the client says dead. The client
had two usable replicas for reads and no way to prefer them; a failover
could only be forced by an operator.

## Implications for redis-rs

- Default `TCP_NODELAY` to on.
- Track per-node latency/timeouts; a node that times out N times in a row
  should be treated as down by the client (fail fast, reads to replicas,
  reconnect) even while the cluster still lists it as master.
- Retrying a timed-out request on the same node immediately (E3, E6) only
  burns the overall budget.
