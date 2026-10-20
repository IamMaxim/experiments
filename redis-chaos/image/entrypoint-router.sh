#!/bin/sh
# Base ruleset: one forward chain the chaos CLI appends drop rules to.
# The router's own traffic (OUTPUT/INPUT) is never filtered, so it can always
# observe every node.
set -e
nft -f - <<'NFT'
table inet chaos {
  chain incidents {
    type filter hook forward priority filter; policy accept;
  }
}
NFT
exec sleep infinity
