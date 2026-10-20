TITLE="self-heal BOTH: SIGSTOP master dc3-s3 (A4)"
HOLD=30
CLIENT_ARGS="--nodelay --self-heal both"
incident() { bin/chaos pause dc3-s3; }
heal() { bin/chaos unpause dc3-s3; }
