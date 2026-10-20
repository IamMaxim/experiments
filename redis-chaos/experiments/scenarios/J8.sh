TITLE="self-heal BOTH + TCP_USER_TIMEOUT 2s: blackhole master dc1-s1 for 60s (A1)"
HOLD=60
CLIENT_ARGS="--nodelay --self-heal both --tcp-user-timeout-ms 2000"
incident() { bin/chaos isolate dc1-s1; }
