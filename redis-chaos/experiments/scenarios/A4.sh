TITLE="SIGSTOP master dc3-s3 (hung process)"
HOLD=30
incident() { bin/chaos pause dc3-s3; }
heal() { bin/chaos unpause dc3-s3; }
