TITLE="reshard 2000 slots s1 -> s2 under load, then back"
HOLD=60
incident() {
  local a b; a=$(bin/chaos cli dc1-s1 cluster myid); b=$(bin/chaos cli dc2-s2 cluster myid)
  docker exec rc-router redis-cli --cluster reshard 10.77.11.10:6379 --cluster-from "$a" --cluster-to "$b" --cluster-slots 2000 --cluster-yes >/dev/null && echo "moved 2000 slots to s2"
  sleep 10
  docker exec rc-router redis-cli --cluster reshard 10.77.11.10:6379 --cluster-from "$b" --cluster-to "$a" --cluster-slots 2000 --cluster-yes >/dev/null && echo "moved 2000 slots back"
}
heal() { :; }
