TITLE="one-way: packets from master dc1-s1 to everyone are dropped"
HOLD=30
incident() { bin/chaos cut-oneway dc1-s1 all,client; }
