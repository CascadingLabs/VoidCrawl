#!/usr/bin/env bash
# Run viewerctl in the single running headful Compose service.
set -euo pipefail

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
COMPOSE_FILE="$SCRIPT_DIR/docker-compose.headful.yml"

mapfile -t containers < <(docker compose -f "$COMPOSE_FILE" ps -q --status running)
case "${#containers[@]}" in
    1) ;;
    0)
        echo "no running headful Compose service; start one with ./docker/run-headful.sh" >&2
        exit 1
        ;;
    *)
        echo "more than one headful Compose service is running; stop all but one before opening a viewer" >&2
        exit 1
        ;;
esac

exec docker exec "${containers[0]}" viewerctl "$@"
