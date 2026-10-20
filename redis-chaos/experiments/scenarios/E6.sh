TITLE="60% packet loss into master dc1-s1"
HOLD=40
incident() { bin/chaos netem dc1-s1 loss 60%; }
