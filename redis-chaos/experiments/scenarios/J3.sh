TITLE="self-heal BOTH: blackhole master dc1-s1 for 60s (A1)"
HOLD=60
CLIENT_ARGS="--nodelay --self-heal both"
incident() { bin/chaos isolate dc1-s1; }
