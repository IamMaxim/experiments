TITLE="self-heal POKE: blackhole master dc1-s1 for 60s (A1)"
HOLD=60
CLIENT_ARGS="--nodelay --self-heal poke"
incident() { bin/chaos isolate dc1-s1; }
