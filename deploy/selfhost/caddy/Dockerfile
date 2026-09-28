# Caddy with the Route 53 DNS provider (Let's Encrypt DNS-01 challenge —
# works without port 80 reachable) and the dynamic DNS app (keeps the
# domain's A record pointed at this network's changing public IP).
FROM caddy:2-builder AS builder
RUN xcaddy build \
    --with github.com/caddy-dns/route53 \
    --with github.com/mholt/caddy-dynamicdns

FROM caddy:2
COPY --from=builder /usr/bin/caddy /usr/bin/caddy
