TITLE="self-heal BOTH: kill -9 master dc2-s2 (A3 regression check)"
HOLD=30
CLIENT_ARGS="--nodelay --self-heal both"
incident() { bin/chaos kill dc2-s2; }
heal() { bin/chaos start dc2-s2; }
