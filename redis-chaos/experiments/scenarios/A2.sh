TITLE="master dc1-s1 cut off with ICMP reject"
HOLD=30
incident() { REJECT=1 bin/chaos isolate dc1-s1; }
