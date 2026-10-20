TITLE="dc1 flapping 8s off / 8s on, 4 times (above node-timeout)"
HOLD=64
incident() { for i in $(seq 4); do bin/chaos isolate dc1; sleep 8; bin/chaos heal "isolate dc1"; sleep 8; done; }
