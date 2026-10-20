TITLE="pipelines while master dc2-s2 is killed"
HOLD=30
LOAD_ARGS="--mode pipe"
incident() { bin/chaos kill dc2-s2; }
heal() { bin/chaos start dc2-s2; }
