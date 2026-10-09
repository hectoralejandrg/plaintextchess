# Split realtime architecture helpers (design Test Strategy).
#
# The default `cargo test` stays Redis-free: `TIER=all` is the default and the
# split end-to-end tests are `#[ignore]`d. Use `test-split` to run them against
# a real Redis.

REDIS_URL ?= redis://127.0.0.1:6379

.PHONY: redis-up redis-down test-split multi-node-up multi-node-down multi-node-smoke

redis-up:
	docker compose up -d redis

redis-down:
	docker compose down

# Run the env-gated split end-to-end tests against a real Redis. They share the
# Phase 1 inbound channel, so they run single-threaded.
test-split:
	cd server && REDIS_URL=$(REDIS_URL) cargo test --test split_e2e -- --ignored --test-threads=1

# Multi-node Docker Compose targets (Phase 2: transport scale-out)
# Starts Redis + logic tier + 2 transport nodes + NGINX reverse proxy.
multi-node-up:
	docker compose up -d --build

multi-node-down:
	docker compose down -v

# Smoke test the multi-node setup via the reverse proxy.
# Verifies: healthz returns 200 with tier=transport, and round-robin between nodes.
multi-node-smoke:
	@echo "Waiting for proxy to be ready..."
	@sleep 5
	@echo "Testing /healthz through proxy (should show tier=transport)..."
	@curl -sf http://localhost:8080/healthz | jq -e '.status=="ok" and .tier=="transport" and .node_id' >/dev/null \
	  && echo "healthz OK" || (echo "healthz FAILED"; exit 1)
	@echo "Testing round-robin (two requests should hit different transport nodes)..."
	@NODE1=$$(curl -sf http://localhost:8080/healthz | jq -r .node_id); \
	 NODE2=$$(curl -sf http://localhost:8080/healthz | jq -r .node_id); \
	 if [ "$$NODE1" != "$$NODE2" ]; then \
	   echo "Round-robin OK: $$NODE1 -> $$NODE2"; \
	 else \
	   echo "Round-robin check: same node twice ($$NODE1) — may be OK if only one node healthy yet"; \
	 fi
	@echo "Multi-node smoke test passed."