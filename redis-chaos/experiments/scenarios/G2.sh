TITLE="blackhole dc1-s1 with 16 retries and no overall timeout"
HOLD=30
CLIENT_ARGS="--nodelay --retries 16 --overall-timeout-ms 0"
incident() { bin/chaos isolate dc1-s1; }
