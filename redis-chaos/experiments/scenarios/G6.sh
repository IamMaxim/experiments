TITLE="blackhole dc1-s1 with TCP_USER_TIMEOUT 2s"
HOLD=30
CLIENT_ARGS="--nodelay --tcp-user-timeout-ms 2000"
incident() { bin/chaos isolate dc1-s1; }
