TITLE="split brain as D4, no overall timeout (see the raw server errors)"
HOLD=30
CLIENT_ARGS="--nodelay --overall-timeout-ms 0"
LOAD_ARGS="--keys 100000"
incident() { bin/chaos cut dc1-s1 dc1-s2,dc1-s3,dc2,dc3; }
