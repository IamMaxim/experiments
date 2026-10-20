TITLE="1500ms delay into master dc1-s1 (over the response timeout)"
HOLD=30
incident() { bin/chaos netem dc1-s1 delay 1500ms; }
