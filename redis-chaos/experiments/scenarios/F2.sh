TITLE="scale out: add dc1-s4 as a 4th master and rebalance; then drain and remove it"
HOLD=90
TAIL=20
incident() {
  docker exec rc-router redis-cli --cluster add-node 10.77.14.10:6379 10.77.11.10:6379 >/dev/null && echo "added dc1-s4"
  sleep 5
  docker exec rc-router redis-cli --cluster rebalance 10.77.11.10:6379 --cluster-use-empty-masters >/dev/null && echo "rebalanced slots onto dc1-s4"
  sleep 20
  local id; id=$(bin/chaos cli dc1-s4 cluster myid)
  docker exec rc-router redis-cli --cluster rebalance 10.77.11.10:6379 --cluster-weight "$id=0" >/dev/null && echo "drained dc1-s4"
  sleep 5
  docker exec rc-router redis-cli --cluster del-node 10.77.11.10:6379 "$id" >/dev/null && echo "removed dc1-s4"
}
heal() { :; }
post() { bin/chaos cli dc1-s4 cluster reset hard >/dev/null; }
