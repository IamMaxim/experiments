TITLE="self-heal BOTH, all 9 seeds: client cut from seed masters, masters moved (D5)"
HOLD=40
CLIENT_ARGS="--nodelay --self-heal both --seeds 10.77.11.10:6379,10.77.12.10:6379,10.77.13.10:6379,10.77.21.10:6379,10.77.22.10:6379,10.77.23.10:6379,10.77.31.10:6379,10.77.32.10:6379,10.77.33.10:6379"
incident() {
  bin/chaos cut client dc1-s1,dc2-s2,dc3-s3; sleep 10
  for n in dc2-s1 dc3-s2 dc1-s3; do bin/chaos cli "$n" cluster failover; done
}
