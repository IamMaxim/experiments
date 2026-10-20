TITLE="restart all 9 nodes at once"
HOLD=30
incident() { bin/chaos restart all; }
heal() { :; }
