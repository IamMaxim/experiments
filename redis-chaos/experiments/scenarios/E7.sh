TITLE="100ms delay into the client (slow client link)"
HOLD=20
incident() { bin/chaos netem client delay 100ms; }
