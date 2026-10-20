TITLE="split brain as D4, every key written at most once (loss not masked by later writes)"
HOLD=30
LOAD_ARGS="--keys 100000"
incident() { bin/chaos cut dc1-s1 dc1-s2,dc1-s3,dc2,dc3; }
