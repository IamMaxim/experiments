TITLE="split brain: master dc1-s1 cut from all nodes but still reachable by the client"
HOLD=30
incident() { bin/chaos cut dc1-s1 dc1-s2,dc1-s3,dc2,dc3; }
