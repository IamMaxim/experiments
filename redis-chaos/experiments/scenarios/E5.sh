TITLE="25% packet loss into master dc1-s1"
HOLD=30
incident() { bin/chaos netem dc1-s1 loss 25%; }
