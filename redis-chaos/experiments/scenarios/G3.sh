TITLE="blackhole dc1-s1, mixed load, reads from replicas"
HOLD=30
CLIENT_ARGS="--nodelay --read-replicas"
LOAD_ARGS="--mode mixed"
incident() { bin/chaos isolate dc1-s1; }
