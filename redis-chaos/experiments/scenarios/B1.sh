TITLE="blackhole replica dc2-s1, master-only reads"
HOLD=30
LOAD_ARGS="--mode mixed"
incident() { bin/chaos isolate dc2-s1; }
