TITLE="blackhole replica dc2-s1, reads from replicas"
HOLD=30
CLIENT_ARGS="--nodelay --read-replicas"
LOAD_ARGS="--mode mixed"
incident() { bin/chaos isolate dc2-s1; }
