TITLE="self-heal BOTH: CLIENT PAUSE WRITE 30s on dc1-s1 (A8)"
HOLD=30
CLIENT_ARGS="--nodelay --self-heal both"
LOAD_ARGS="--mode mixed"
incident() { bin/chaos cli dc1-s1 client pause 30000 write; }
heal() { bin/chaos cli dc1-s1 client unpause; }
