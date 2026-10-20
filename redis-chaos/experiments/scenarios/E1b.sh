TITLE="100ms delay into every node, Nagle on (library default)"
HOLD=20
CLIENT_ARGS=""
incident() { bin/chaos netem all delay 100ms; }
