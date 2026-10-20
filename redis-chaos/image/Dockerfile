# One image for redis nodes, the router and the client box, so redis-cli
# versions match everywhere.
FROM redis:8.2.2
RUN apt-get update \
 && apt-get install -y --no-install-recommends iproute2 nftables iputils-ping tcpdump procps netcat-openbsd \
 && rm -rf /var/lib/apt/lists/*
COPY entrypoint-node.sh entrypoint-router.sh entrypoint-client.sh /usr/local/bin/
