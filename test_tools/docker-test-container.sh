#!/bin/bash

# Flicker Docker Log Test Container
# This script runs a Docker container that generates logs for testing

set -e

CONTAINER_NAME="flicker-test-logger"
LOG_INTERVAL="${LOG_INTERVAL:-2}"  # Default: log every 2 seconds

echo "Starting Docker test container: $CONTAINER_NAME"
echo "Log interval: ${LOG_INTERVAL}s"
echo ""
echo "The container will generate logs with various levels:"
echo "  - INFO messages"
echo "  - WARN messages"
echo "  - ERROR messages"
echo "  - DEBUG messages (for testing filters)"
echo ""
echo "To test with Flicker, use: cargo run -- -c test_tools/docker-test-config.yaml"
echo ""
echo "Press Ctrl+C to stop the container"
echo ""

# Clean up any existing container with the same name
docker rm -f $CONTAINER_NAME 2>/dev/null || true

# Run the container
docker run --name $CONTAINER_NAME --rm alpine sh -c "
counter=1
while true; do
  echo \"[\$(date -Iseconds)] INFO  - Application started successfully (message \$counter)\"
  sleep $LOG_INTERVAL

  echo \"[\$(date -Iseconds)] DEBUG - Processing request (message \$counter)\"
  sleep $LOG_INTERVAL

  echo \"[\$(date -Iseconds)] WARN  - High memory usage: \$(( 75 + RANDOM % 15 ))% (message \$counter)\"
  sleep $LOG_INTERVAL

  if [ \$(( counter % 5 )) -eq 0 ]; then
    echo \"[\$(date -Iseconds)] ERROR - Failed to connect to database: timeout (message \$counter)\"
  else
    echo \"[\$(date -Iseconds)] INFO  - Request completed successfully (message \$counter)\"
  fi

  sleep $LOG_INTERVAL
  counter=\$((counter + 1))
done
"

# This line is only reached when the container stops (Ctrl+C)
echo ""
echo "Test container stopped"
