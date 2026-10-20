# experiments

Reproducible lab setups and the results they produced. Each directory is
self-contained and has its own README.

| directory | what |
|---|---|
| [`redis-chaos/`](redis-chaos/) | Redis Cluster (3 DCs × 3 shards) under infrastructure incidents, with a redis-rs client: the lab, 64 scenarios, raw runs, reports, and an app-side self-healing workaround. |

Clone with submodules:

```sh
git clone --recurse-submodules https://github.com/IamMaxim/experiments.git
```

MIT, see [LICENSE](LICENSE).
