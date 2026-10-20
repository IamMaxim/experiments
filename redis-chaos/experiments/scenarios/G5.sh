TITLE="blackhole dc1-s1, open-loop load, per-node concurrency limit 8"
HOLD=30
CLIENT_ARGS="--nodelay --concurrency-limit 8"
LOAD_ARGS="--open-loop"
incident() { bin/chaos isolate dc1-s1; }
