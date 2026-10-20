TITLE="docker restart of master dc1-s1 (back within node-timeout)"
HOLD=20
incident() { bin/chaos restart dc1-s1; }
heal() { :; }
