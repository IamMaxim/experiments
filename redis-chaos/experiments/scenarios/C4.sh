TITLE="client isolated together with dc1 (client in the minority DC)"
HOLD=30
incident() { bin/chaos isolate dc1,client; }
