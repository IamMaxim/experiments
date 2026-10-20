TITLE="CLIENT KILL TYPE normal on every master, 4 times 5s apart"
HOLD=25
incident() { for i in $(seq 4); do for n in dc1-s1 dc2-s2 dc3-s3; do bin/chaos cli "$n" client kill type normal >/dev/null; done; echo "killed clients on all masters"; sleep 5; done; }
heal() { :; }
