TITLE="one-way: packets from everyone to master dc1-s1 are dropped"
HOLD=30
incident() { bin/chaos cut-oneway all,client dc1-s1; }
