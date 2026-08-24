#!/bin/bash
set -euo pipefail

COMPOSE_FILE=".ci/stack.yml"
STACK_NAME="leetcodeview"
ENV_PATH="/mnt/ssd/@docker/leetcodeview/.env"

set -a
source "$ENV_PATH"
set +a


echo "Pulling latest images..."
if ! docker compose -f "$COMPOSE_FILE" pull; then
    echo "Error: Image pull failed."
    exit 1
fi

echo "Deploying stack: $STACK_NAME (rolling update)"
if ! docker stack deploy --prune --with-registry-auth -c "$COMPOSE_FILE" "$STACK_NAME"; then
    echo "Error: Stack deployment failed."
    exit 1
fi

echo "Deployment complete."
exit 0
