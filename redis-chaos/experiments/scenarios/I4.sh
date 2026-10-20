TITLE="new connections while cluster_state is fail (dc1 and dc2 isolated)"
HOLD=25
LOAD_CMD=connect-probe
LOAD_ARGS="--count 45 --interval-ms 1000"
incident() { bin/chaos isolate dc1; bin/chaos isolate dc2; }
