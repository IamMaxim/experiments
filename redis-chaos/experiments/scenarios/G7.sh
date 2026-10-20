TITLE="blackhole dc1-s1 with TCP keepalive 1s"
HOLD=30
CLIENT_ARGS="--nodelay --keepalive-ms 1000"
incident() { bin/chaos isolate dc1-s1; }
