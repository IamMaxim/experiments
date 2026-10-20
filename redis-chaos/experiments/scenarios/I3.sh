TITLE="new connections while the client cannot reach any seed (cluster healthy)"
HOLD=20
LOAD_CMD=connect-probe
LOAD_ARGS="--count 40 --interval-ms 1000"
incident() { bin/chaos cut client dc1-s1,dc2-s2,dc3-s3; }
