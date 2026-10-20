TITLE="client cut from the 3 seed masters, then masters moved to the replicas"
HOLD=40
incident() {
  bin/chaos cut client dc1-s1,dc2-s2,dc3-s3; sleep 10
  for n in dc2-s1 dc3-s2 dc1-s3; do bin/chaos cli "$n" cluster failover; done
}
