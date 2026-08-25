#!/bin/bash
set -euo pipefail

# Not `docker stack deploy` anymore — see .ci/leetcodeview-api.service's
# own header comment for the full validated reason: Docker Swarm has no
# way to grant `cgroupns=host`/`security_opt` overrides, both genuinely
# required for nsjail to isolate untrusted code on a real Linux host
# (confirmed empirically, not assumed). The api container now runs as a
# plain systemd-managed `docker run`, outside Swarm's service model.
IMAGE="pizito:5001/leetcodeview:latest"
SERVICE="leetcodeview-api.service"

echo "Pulling latest image..."
if ! docker pull "$IMAGE"; then
    echo "Error: Image pull failed."
    exit 1
fi

echo "Restarting $SERVICE..."
if ! sudo systemctl restart "$SERVICE"; then
    echo "Error: Service restart failed."
    exit 1
fi

echo "Waiting for health check..."
for _ in $(seq 1 30); do
    if sudo systemctl is-active --quiet "$SERVICE" && \
       docker exec leetcodeview-api curl -sf http://127.0.0.1:8080/health >/dev/null 2>&1; then
        echo "Deployment complete."
        exit 0
    fi
    sleep 2
done

echo "Error: $SERVICE did not become healthy in time."
sudo systemctl status "$SERVICE" --no-pager || true
exit 1
