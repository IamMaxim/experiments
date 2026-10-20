TITLE="CLIENT PAUSE WRITE 30s on master dc1-s1 (slow node, bus alive)"
HOLD=30
LOAD_ARGS="--mode mixed"
incident() { bin/chaos cli dc1-s1 client pause 30000 write; }
heal() { bin/chaos cli dc1-s1 client unpause; }
