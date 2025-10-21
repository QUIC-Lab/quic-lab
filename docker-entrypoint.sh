#!/bin/sh
set -e

# start nginx as root (needed for :80)
nginx -g 'daemon off;' &
NGINX_PID=$!

# run Rust app as dedicated non-root user
su -s /bin/sh appuser -c "/app/quic-lab${1:+ \"\$@\"}" &
APP_PID=$!

term() { kill -TERM "$NGINX_PID" "$APP_PID" 2>/dev/null || true; }
trap term INT TERM

# portable wait loop
while kill -0 "$NGINX_PID" 2>/dev/null && kill -0 "$APP_PID" 2>/dev/null; do
  sleep 1
done
term
wait
