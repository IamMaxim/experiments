TITLE="manual CLUSTER FAILOVER to replica dc2-s1"
HOLD=15
incident() { bin/chaos cli dc2-s1 cluster failover; }
heal() { :; }
