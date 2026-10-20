TITLE="dc1 flapping 3s off / 3s on, 8 times (below node-timeout)"
HOLD=48
incident() { for i in $(seq 8); do bin/chaos isolate dc1; sleep 3; bin/chaos heal "isolate dc1"; sleep 3; done; }
