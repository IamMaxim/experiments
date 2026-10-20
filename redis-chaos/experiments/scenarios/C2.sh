TITLE="isolate dc1 and dc2: only dc3 left, no master quorum"
HOLD=30
incident() { bin/chaos isolate dc1; bin/chaos isolate dc2; }
