TITLE="server drops idle connections (CONFIG SET timeout 1) under a 1 op/s load"
HOLD=30
LOAD_ARGS="--rate 1 --workers 1"
incident() { for n in $(bin/chaos targets all); do bin/chaos cli "$n" config set timeout 1 >/dev/null; done; echo "timeout 1 everywhere"; }
heal() { for n in $(bin/chaos targets all); do bin/chaos cli "$n" config set timeout 0 >/dev/null; done; echo "timeout 0 everywhere"; }
