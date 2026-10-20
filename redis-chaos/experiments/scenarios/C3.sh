TITLE="isolate dc1 and dc2 with cluster-require-full-coverage no"
HOLD=30
pre() { for n in $(bin/chaos targets all); do bin/chaos cli "$n" config set cluster-require-full-coverage no >/dev/null; done; }
post() { for n in $(bin/chaos targets all); do bin/chaos cli "$n" config set cluster-require-full-coverage yes >/dev/null; done; }
incident() { bin/chaos isolate dc1; bin/chaos isolate dc2; }
