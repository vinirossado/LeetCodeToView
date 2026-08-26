#!/bin/bash
set -euo pipefail

# Deploy: builds both images LOCALLY (no registry) and restarts both
# systemd units. Run from the repo root on the deploy host itself, after a
# `git pull` — see .ci/readme.md for one-time setup.

echo "Building api image..."
docker build -f .ci/Dockerfile -t leetcodeview:latest .

echo "Building frontend image..."
docker build -f .ci/Dockerfile.frontend -t leetcodeview-fe:latest .

echo "Restarting leetcodeview-api.service..."
sudo systemctl restart leetcodeview-api.service

echo "Waiting for api health check..."
api_healthy=false
for _ in $(seq 1 30); do
    if docker exec leetcodeview-api curl -sf http://127.0.0.1:8080/health >/dev/null 2>&1; then
        api_healthy=true
        break
    fi
    sleep 2
done
if [ "$api_healthy" != true ]; then
    echo "Error: api did not become healthy in time."
    sudo systemctl status leetcodeview-api.service --no-pager || true
    exit 1
fi

echo "Restarting leetcodeview-fe.service..."
sudo systemctl restart leetcodeview-fe.service

echo "Waiting for frontend health check..."
for _ in $(seq 1 30); do
    if curl -sf http://127.0.0.1:8081/ >/dev/null 2>&1; then
        echo "Deployment complete."
        exit 0
    fi
    sleep 2
done

echo "Error: frontend did not become healthy in time."
sudo systemctl status leetcodeview-fe.service --no-pager || true
exit 1
