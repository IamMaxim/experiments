TITLE="graceful stop (SIGTERM) of master dc1-s1, start after 30s"
HOLD=30
incident() { bin/chaos stop dc1-s1; }
heal() { bin/chaos start dc1-s1; }
