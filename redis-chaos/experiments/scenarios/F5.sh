TITLE="rolling restart of all 9 nodes, 10s apart"
HOLD=95
TAIL=20
incident() { for n in $(bin/chaos targets all); do bin/chaos restart "$n"; sleep 10; done; }
heal() { :; }
