TITLE="100ms delay into every node, TCP_NODELAY on"
HOLD=20
incident() { bin/chaos netem all delay 100ms; }
