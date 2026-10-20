# F. Cluster operations

| run | operation | errors | lost | notes |
|---|---|---|---|---|
| F1 `F1-20261020-173545` | reshard 2000 slots s1 → s2 under load, then back | **0** | 0 | max latency 20ms; 32016 ok |
| F2 `F2-20261020-173711` | add dc1-s4 as 4th master, rebalance onto it, drain, `del-node` | **0** | 0 | 46016 ok, max 5.8ms |
| F4 `F4-20261020-173913` | restart all 9 nodes at once | 36 | 0 | 5 `broken pipe`, 20 timeouts, **11 MOVED surfaced**; ~8s to recover |
| F5 `F5-20261020-174008` | rolling restart, 10s apart | 48 | 0 | 16 per master restart: `CLUSTERDOWN` ×40, `Connection refused` ×7, MOVED ×1 |
| F6 `F6-20261020-174213` | server `timeout 1` (idle connections closed), 1 op/s | **9 of 51** | 0 | 8 `broken pipe` + 1 MOVED surfaced |
| F7 `F7-20261020-174309` | `CLIENT KILL TYPE normal` on all masters ×4 | **0** | 0 | 18016 ok |

## F1 / F2: slot migration and scale out/in

Both clean. ASK redirects were followed, MOVED after each slot's migration
triggered a refresh, the new node's address was connected on demand, and the
removed node disappeared from the map at the next refresh. This is the path
the library is built around. (The reshard of 2000 nearly-empty slots took
~10s each way; a loaded cluster would hold the ASK phase longer, which H/I
do not cover.)

## F4: full restart

All connections drop at 5.7s. The nodes come back over the next ~8s, each
one `cluster_state fail` until it has seen the others. The client: `broken
pipe` for the requests in flight at the instant of the drop, then the A5
pattern (replica says MOVED to a master that is not back yet; the redirect
fails; after 3 retries the MOVED is returned to the caller: 11 of them), then
timeouts while the slot refresh dials nodes that are not up yet (1s each,
under the write lock). 36 errors, recovered at ~13s; no lost writes (AOF).

## F5: rolling restart

Each replica restart was invisible. Each master restart cost exactly one
batch of 16 errors: `Connection refused` while the port was closed (returned
to the caller after the retries, since the fallback replica said MOVED to
the restarting node and the redirect failed), and `CLUSTERDOWN` from the
node during its first second back (the ≥1.28s retry sleep does not fit the
3s budget twice). Recovery ~2s each time, no failover (restart within
node-timeout).

## F6: server closes idle connections

With a 1 op/s load and `timeout 1`, every connection is closed by the server
between uses. The library noticed each close ("connection lost; repairing in
background") and reconnected, but **8 of the 51 requests were returned to
the caller as `broken pipe`**, and one as MOVED. Each of those coincided
with several connections being closed at once and a slot refresh running
(the refresh itself used a dead connection and failed; `send_refresh_error`
then fails every in-flight request with that error, and each such failure
consumes one retry). Unverified which of the two paths produced the final
error; either way a server-side idle close must not reach the application
as an I/O error on a fresh request.

## F7: CLIENT KILL

Killing every client connection on all three masters four times produced no
errors at all: the in-flight request failed with a reset, was retried,
and the reconnect happened before the next request. The difference from F6
is timing: here the drop is noticed by a request that is already in flight
on a known-dead connection; in F6 the drop is noticed by a refresh that
runs over a dead connection.

## Implications for redis-rs

- A refresh must not use a connection that just reported an error, and a
  failed refresh must not fail user requests (keep them pending, retry the
  refresh).
- A redirect to an address that is unreachable should be followed by a
  refresh from another node, not by returning MOVED to the caller.
- Lower CLUSTERDOWN backoff (as in A6).
