# G. Client knobs, same incident: blackhole master dc1-s1 for 30s

Baseline A1/A2 (1s response, 3s overall, 3 retries): 240 errors, shard 1
dead from 8s until heal + TCP backoff, no refresh.

| run | knob | errors | shard 1 outage | recovered |
|---|---|---|---|---|
| G1 `G1-20261020-174407` | library defaults: 16 retries, overall = response (1s) | 674 | 7–51s | after heal + backoff (refresh at 62s) |
| G2 `G2-20261020-174516` | 16 retries, no overall timeout | 33, **max latency 17.2s** | 24–60s | 60.4s |
| G3 `G3-20261020-174622` | `--read-replicas`, mixed load | 234 | writes dead; reads served when a worker got one | after heal |
| G4 `G4-20261020-174721` | open-loop load | 3076 (every shard-1 op) | 9–53s | 58.4s |
| G5 `G5-20261020-174827` | open-loop + `concurrency_limit 8` | 3114 | same | same |
| G6 `G6-20261020-174933` | **`TCP_USER_TIMEOUT 2s`** | **22** | **8–13s** | **13.6s, before the heal** |
| G7 `G7-20261020-175029` | TCP keepalive 1s | 240 | 9–51s | after heal + backoff |

## G1: library defaults

With the overall timeout equal to the response timeout, the first attempt
uses up the whole budget; the 16 retries never run. Requests fail after 1s
instead of 3s, so the harness produced more errors (674), and the healthy
shards also saw 11 timeouts at 14s. Same stuck-on-stale-master behaviour,
same post-heal backoff.

## G2: no overall timeout

Every shard-1 request made 17 attempts of 1s each to the same dead node and
failed after ~17s. The first error appeared at 24s. Few errors (33), but the
application saw 17s latencies, and recovery was still gated by the heal.
Retrying a timeout on the same connection has no value at any retry count.

## G3: reads from replicas

Reads of shard 1 went to dc2-s1/dc3-s1 and succeeded whenever the harness
got to them; writes were stuck as in A1. With a closed-loop harness the
stuck writes starved the reads (s1 ok counts in the single digits per
second), so this run says more about the harness than the library. Verdict:
replica reads do survive a master blackhole, and nothing moves the writes.

## G4 / G5: open loop

With requests sent regardless of outstanding ones, shards 2 and 3 served
their full share (318–354 ok/s of the 400 target) throughout; shard 1 was
100% errors for the whole incident (3076 timeouts ≈ 73/s, with ~170 requests
permanently in flight waiting for their 3s). `connection_concurrency_limit 8`
changed nothing: the limit caps requests per connection, but the queued
requests wait for a permit and then time out in turn.

So the real availability picture is: one shard out, two shards fine. The
closed-loop collapse in A1 is the application's call pattern, and it is a
common one (a handler that does several sequential Redis calls).

## G6: TCP_USER_TIMEOUT

The kernel gave up on the unacknowledged data 2s after the cut and aborted
the socket. redis-rs saw the error at 7.3s (`connection lost; repairing in
background`), and from there the A3/A5 path ran: fallback to a replica,
MOVED to the dead address, 7 MOVED errors surfaced once retries ran out,
refresh storm with the 1s lock stall, and at 13.6s the refresh found the
new master. **22 errors, outage 5s, no heal needed.** After the heal the
old socket was already gone, so no backoff wait either.

This is a one-line setting (`TcpSettings::set_user_timeout`) that turns the
worst case in this whole matrix into the kill -9 case. It does not replace
timeout-aware routing (A8, D1, E3 still need that), but it removes the
"silent socket" class entirely.

## G7: keepalive

Identical to A1. Keepalive probes are only sent on an idle connection; with
data in flight the retransmit timer governs, so a 1s keepalive does nothing
here. It would help an idle connection notice a dead peer (not measured).

## Implications for redis-rs

- Set `TCP_USER_TIMEOUT` by default on Linux (e.g. to the response timeout,
  or a few seconds), or close a connection after N consecutive timeouts to
  get the same effect portably.
- The MOVED-to-dead-address leak (A5) shows up again here; fix it.
- `overall_response_timeout` defaulting to `response_timeout` makes retries
  dead code; the defaults should be coherent.
