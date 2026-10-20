# redis-chaos

A lab for infrastructure-incident testing of Redis Cluster with a redis-rs client.

## Topology

3 virtual DCs × 3 shards. Each shard has its master in a different DC and one
replica in each of the other two, so every DC holds exactly one node per shard.

| shard | slots        | master (designed) | replicas         |
|-------|--------------|-------------------|------------------|
| s1    | 0-5460       | dc1-s1            | dc2-s1, dc3-s1   |
| s2    | 5461-10922   | dc2-s2            | dc1-s2, dc3-s2   |
| s3    | 10923-16383  | dc3-s3            | dc1-s3, dc2-s3   |

- Node `dcD-sS` is container `rc-dcD-sS` at `10.77.DS.10`.
- The client box `rc-client` is at `10.77.100.10`.
- `rc-router` holds `.254` on every subnet.

Every node sits alone in its own internal /24 with its default route through
the router. All traffic is routed (ttl 63), including traffic between two
nodes of the same DC, so the router can drop or delay any pair. The router's
own traffic is never filtered, so `chaos status` sees every node's view even
during a partition.

Redis 8.2.2, `cluster-node-timeout 5000`, AOF on (`conf/redis.conf`).

## Quick start

```sh
git submodule update --init             # redis-rs, pinned to the tested commit
bin/chaos reset                         # build, start, create the cluster
(cd client && cargo build --release)    # static musl binary, mounted into rc-client
bin/chaos client info
bin/chaos client load --duration 30 --rate 500
bin/chaos status
```

## Incidents

Targets: `dc1`, `s2` (a shard across all DCs), `dc3-s1` (one node), `client`,
`all`, or a comma list (`dc1-s1,dc2-s1`).

```sh
bin/chaos isolate dc1                   # dc1 only talks to itself
bin/chaos isolate dc2-s2                # one node fully cut off
bin/chaos cut dc1 dc2                   # partition between two DCs; dc3 sees both
bin/chaos cut-oneway dc1 dc3            # asymmetric
bin/chaos cut client dc1                # only the client loses dc1
REJECT=1 bin/chaos isolate dc1          # refuse with ICMP instead of blackholing
bin/chaos netem dc3 delay 100ms 20ms loss 1%   # packets entering dc3
bin/chaos kill|stop|start|pause|unpause|restart dc2-s2
bin/chaos incidents                     # what is active
bin/chaos heal                          # remove everything, restart/unpause containers
bin/chaos heal "cut dc1 dc2"            # remove one labelled incident
bin/chaos rebalance                     # restore the designed layout (masters and replica attachments)
bin/chaos cli dc2-s1 cluster failover   # redis-cli against one node, from the router
bin/chaos targets dc1                   # expand a target to node names
```

`dcN-s4` are spare nodes (started, not in the cluster) for add/remove-node
tests.

`netem` with jitter reorders packets, which adds TCP retransmit effects on
top of the delay.

## Client (`client/`, binary `rcc`)

Built on the redis-rs submodule in `redis-rs/` (path dependency, async cluster,
pinned to the commit the experiments ran against).

- `load` runs a workload (`--mode set|get|mixed|txn|pipe|fanout`), closed-loop
  (`--workers`) or `--open-loop`. It prints one line per second with ok,
  errors by kind, p50/p99/max, and ok/err per shard (by designed slot range).
- At exit it reads every key back through masters. Any value below the last
  acknowledged one is counted as LOST; use `--keys 100000` so each key is
  written at most once and a lost window is not masked by later writes.
- `connect-probe` opens fresh connections repeatedly and times them.
- Client knobs: timeouts, retries, `--nodelay`, `--read-replicas`,
  `--concurrency-limit`, `--tcp-user-timeout-ms`, `--keepalive-ms`; see
  `rcc --help`. `RUST_LOG=redis=debug` prints the library's log on the same
  clock as the load lines.
- `--self-heal poke|rebuild|both` (with `--heal-after`, `--heal-interval-ms`)
  turns on the app-side `Healer`: after N consecutive failures on a shard it
  pokes a replica of that shard by address (the MOVED reply makes redis-rs
  refresh its slot map) and/or rebuilds the connection from the seeds.
  `HEAL` lines in the load output show what it did. See
  `experiments/reports/J.md` for what it buys.

## Smoke suite

`bin/smoke` runs 8 basic incidents, each under load. The timeline is 5s
clean, 20s incident (`HOLD=`), heal, then 15s more load. It prints one row per
scenario; logs go to `logs/`.

## Experiments

`experiments/PLAN.md` is the matrix (64 scenarios in groups A–J),
`experiments/scenarios/<id>.sh` defines each one, `bin/exp <id>...` runs them
and stores client log, 2s cluster snapshots and a timeline under
`experiments/runs/`. `bin/digest <run>` condenses a run. Reports are in
`experiments/reports/`, the cross-cutting findings in
`experiments/SUMMARY.md`.
