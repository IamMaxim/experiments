TITLE="busy Lua script for 15s on master dc1-s1 (blocks clients and the bus)"
HOLD=20
incident() { bin/chaos cli dc1-s1 eval "local s=tonumber(redis.call('time')[1]); while tonumber(redis.call('time')[1])-s<15 do end return 1" 0; }
heal() { :; }
