TITLE="kill -9 master dc2-s2, leave it dead 60s"
HOLD=60
incident() { bin/chaos kill dc2-s2; }
heal() { bin/chaos start dc2-s2; }
