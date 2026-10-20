TITLE="blackhole dc1-s1, open-loop load (no head-of-line blocking)"
HOLD=30
LOAD_ARGS="--open-loop"
incident() { bin/chaos isolate dc1-s1; }
