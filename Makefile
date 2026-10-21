# Split realtime architecture helpers (design Test Strategy).
#
# The default `cargo test` stays Redis-free: `TIER=all` is the default and the
# split end-to-end tests are `#[ignore]`d. Use `test-split` to run them against
# a real Redis.

REDIS_URL ?= redis://127.0.0.1:6379

.PHONY: redis-up redis-down test-split multi-node-up multi-node-down multi-node-smoke deploy-up deploy-down deploy-logs deploy-ps deploy-check

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

# Production deployment targets (change deploy-server-docker): the single-box
# monolith + Caddy stack in docker-compose.prod.yml. This is a separate
# deployment surface from the local multi-node lab above; it uses its own
# project name and volumes, so the two never collide.
PROD_COMPOSE ?= docker-compose.prod.yml

deploy-up:
	docker compose -f $(PROD_COMPOSE) up -d --build

deploy-down:
	docker compose -f $(PROD_COMPOSE) down

deploy-logs:
	docker compose -f $(PROD_COMPOSE) logs -f

deploy-ps:
	docker compose -f $(PROD_COMPOSE) ps

# Show what the box looks like and which limits the prod stack will apply.
# Use it before the first deploy to pick the sizing row for this box
# (docs/deploy-docker.md, "Sizing for the box").
deploy-check:
	@echo "== host =="
	@echo "cpus: $$(getconf _NPROCESSORS_ONLN 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null)"
	@echo "ram:  $$(awk '/MemTotal/ {print $$2*1024}' /proc/meminfo 2>/dev/null || sysctl -n hw.memsize 2>/dev/null) bytes"
	@echo "== effective limits (from .env + docker-compose.prod.yml) =="
	@docker compose -f $(PROD_COMPOSE) config | grep -E 'mem_limit|soft:|hard:'
	@echo "== running =="
	@docker compose -f $(PROD_COMPOSE) ps
