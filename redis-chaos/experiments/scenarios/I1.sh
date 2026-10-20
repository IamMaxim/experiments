TITLE="new connections while seed dc1-s1 is blackholed"
HOLD=20
LOAD_CMD=connect-probe
LOAD_ARGS="--count 40 --interval-ms 1000"
incident() { bin/chaos isolate dc1-s1; }
