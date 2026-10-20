TITLE="900ms delay into master dc1-s1 (just under the response timeout)"
HOLD=30
incident() { bin/chaos netem dc1-s1 delay 900ms; }
