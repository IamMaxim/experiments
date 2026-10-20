TITLE="new connections while seeds dc1-s1 and dc2-s2 reject (ICMP)"
HOLD=20
LOAD_CMD=connect-probe
LOAD_ARGS="--count 40 --interval-ms 1000"
incident() { REJECT=1 bin/chaos cut client dc1-s1,dc2-s2; }
