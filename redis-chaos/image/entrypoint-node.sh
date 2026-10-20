#!/bin/sh
# Every node sits alone in its own subnet; all traffic leaves via the router.
set -e
ip route replace default via "$GATEWAY"
exec redis-server /etc/redis/redis.conf
