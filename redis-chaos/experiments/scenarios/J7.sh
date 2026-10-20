TITLE="self-heal BOTH: MULTI/EXEC while master dc2-s2 is killed (H1)"
HOLD=30
CLIENT_ARGS="--nodelay --self-heal both"
LOAD_ARGS="--mode txn"
incident() { bin/chaos kill dc2-s2; }
heal() { bin/chaos start dc2-s2; }
