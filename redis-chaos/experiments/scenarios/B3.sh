TITLE="kill replica dc2-s1, reads from replicas"
HOLD=30
CLIENT_ARGS="--nodelay --read-replicas"
LOAD_ARGS="--mode mixed"
incident() { bin/chaos kill dc2-s1; }
heal() { bin/chaos start dc2-s1; }
