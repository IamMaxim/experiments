TITLE="50 new connections per second while master dc2-s2 is killed"
HOLD=20
LOAD_CMD=connect-probe
LOAD_ARGS="--count 40 --interval-ms 1000 --parallel 50"
incident() { bin/chaos kill dc2-s2; }
heal() { bin/chaos start dc2-s2; }
