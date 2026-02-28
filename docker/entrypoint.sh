#!/bin/sh
# Entrypoint for the signalk-rs Docker container.
# Sets up vcan0 (if the kernel supports it) for NMEA 2000 simulation,
# then exec's the main command (signalk-server).

# Try to create vcan0 — silently skip if the kernel lacks vcan support
# (e.g. local dev without modprobe vcan).
if ip link add vcan0 type vcan 2>/dev/null; then
  ip link set vcan0 up
  echo "entrypoint: vcan0 is up"
else
  echo "entrypoint: vcan0 not available (no vcan module?), skipping"
fi

exec "$@"
