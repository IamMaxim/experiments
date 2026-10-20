TITLE="blackhole dc1-s1 with library defaults: 16 retries, overall timeout = response timeout"
HOLD=30
CLIENT_ARGS="--nodelay --retries 16 --overall-library-default"
incident() { bin/chaos isolate dc1-s1; }
