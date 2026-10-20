#!/bin/sh
set -e
ip route replace default via "$GATEWAY"
exec sleep infinity
