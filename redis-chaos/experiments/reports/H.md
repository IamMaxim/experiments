# H. Transactions, pipelines, fan-out while master dc2-s2 is killed

Incident: `kill -9 dc2-s2` at 5s, start at 35s. Failover at ~13s.

| run | workload | errors | lost | recovered |
|---|---|---|---|---|
| H1 `H1-20261020-175133` | MULTI/EXEC, 3 SETs on one hash tag | **5745 `EXECABORT`; shard 2 failing for the whole run, including after the failover and after the heal** | 0 | **never** |
| H2 `H2-20261020-175231` | pipeline of 5 SETs on one hash tag | 29 timeouts | 0 | 16s (as A3) |
| H3 `H3-20261020-175329` | MSET of 3 keys on 3 shards (fan-out) | 32 timeouts | 0 | 12s |

## H1: transactions never recover

From the first second after the kill, every transaction on shard 2 failed
with:

```
Transaction aborted: [(Index 0, error: Moved: 9877 10.77.22.10:6379),
                      (Index 1, error: Moved: 9877 10.77.22.10:6379),
                      (Index 2, error: Moved: 9877 10.77.22.10:6379)]
```

Sequence: dc2-s2's connection drops → `Reconnecting` → the pipeline falls
back to a replica of the shard → the replica queues the three SETs with
MOVED errors and EXEC returns `EXECABORT` → redis-rs maps `ExecAbort` to
`NoRetry` and returns it. The MOVED inside the aborted transaction is never
looked at, so no redirect and **no slot refresh**: the run shows exactly one
`beginning slot refresh` (at 5.0s, from the connection drop) and none after.
The slot map kept pointing at dead dc2-s2 forever. After the failover (13s)
the replica kept saying MOVED to the dead node; after dc2-s2 came back as a
replica (36.6s) it said MOVED to the new master; the client never followed
either. Single-command SETs on the same keys would have recovered at ~15s.

Also at 12–13s shards 1 and 3 had ~200 EXECABORTs while the cluster was in
`cluster_state fail` during the election (the queued commands got
CLUSTERDOWN).

## H2: pipelines

Non-atomic pipelines behaved like single commands: the replica answered
MOVED per command, the pipeline error carried the redirect, redis-rs
refreshed and retried. 29 timeouts during the A3 window, recovery with the
failover.

## H3: fan-out

`MSET` across three slots is split per node; the shard-2 part failed and
the whole command errored (fan-out results are not retried), 32 timeouts in
3s, then the per-node parts followed the refresh triggered by other
requests. Recovered at 12s.

## Implications for redis-rs

- `EXECABORT` must be inspected: if the queued errors are MOVED/ASK, treat
  the transaction as redirected (redirect + refresh + retry the whole
  transaction); if CLUSTERDOWN/TRYAGAIN, `WaitAndRetry`. This is a
  correctness bug, not a tuning issue: a transactional client on this
  version cannot survive any failover.
- Fan-out commands should at least trigger a refresh when a sub-request
  reports MOVED, even if the aggregate is not retried.
