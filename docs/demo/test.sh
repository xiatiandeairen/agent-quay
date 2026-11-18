#!/bin/bash
# Stand-in for a heavy test run: keeps 3 CPU cores busy for about 6 seconds.
echo "Running 128 tests (3 cores)…"
for _ in 1 2 3; do (end=$((SECONDS + 6)); while ((SECONDS < end)); do :; done) & done
wait
echo "128 tests passed"
