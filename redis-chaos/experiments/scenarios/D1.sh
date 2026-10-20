TITLE="client cut off from dc1 only; cluster healthy"
HOLD=30
incident() { bin/chaos cut client dc1; }
