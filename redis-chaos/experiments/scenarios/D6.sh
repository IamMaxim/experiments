TITLE="dc1 <-> dc2 partition, then manual failover to dc2-s1 through it"
HOLD=30
incident() { bin/chaos cut dc1 dc2; sleep 8; bin/chaos cli dc2-s1 cluster failover; }
