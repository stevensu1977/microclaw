#!/usr/bin/env bash
#
# MicroClaw Firecracker SaaS - Proxy Integration Test
#
# Usage:
#   bash scripts/test-proxy.sh
#
# Prerequisites:
#   - Control plane running on localhost:8080
#   - No existing "test-proxy" tenant (will be created and cleaned up)
#

set -euo pipefail

CONTROL_PLANE="http://localhost:8080"
TENANT_ID="test-proxy"
PASSED=0
FAILED=0

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

pass() {
    PASSED=$((PASSED + 1))
    echo -e "  ${GREEN}PASS${NC} $1"
}

fail() {
    FAILED=$((FAILED + 1))
    echo -e "  ${RED}FAIL${NC} $1"
    echo -e "       Expected: $2"
    echo -e "       Got:      $3"
}

section() {
    echo ""
    echo -e "${CYAN}--- $1 ---${NC}"
}

cleanup() {
    echo ""
    echo -e "${YELLOW}Cleaning up tenant '${TENANT_ID}'...${NC}"
    curl -s -X DELETE "${CONTROL_PLANE}/api/v1/tenants/${TENANT_ID}" > /dev/null 2>&1 || true
}

# ============================================================
# Phase 0: Preflight
# ============================================================
section "Preflight checks"

HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" "${CONTROL_PLANE}/health")
if [ "$HTTP_CODE" != "200" ]; then
    echo -e "${RED}Control plane is not running at ${CONTROL_PLANE}${NC}"
    exit 1
fi
pass "Control plane is reachable"

# Clean up any leftover tenant from a previous run
curl -s -X DELETE "${CONTROL_PLANE}/api/v1/tenants/${TENANT_ID}" > /dev/null 2>&1 || true

# ============================================================
# Phase 1: Control Plane API (without proxy)
# ============================================================
section "Control Plane API"

# Test: health endpoint
BODY=$(curl -s "${CONTROL_PLANE}/health")
if echo "$BODY" | grep -q '"ok"'; then
    pass "GET /health returns ok"
else
    fail "GET /health returns ok" '{"status":"ok"}' "$BODY"
fi

# Test: list tenants (should be empty or not contain our test tenant)
BODY=$(curl -s "${CONTROL_PLANE}/api/v1/tenants")
if ! echo "$BODY" | grep -q "\"${TENANT_ID}\""; then
    pass "GET /api/v1/tenants does not contain '${TENANT_ID}'"
else
    fail "GET /api/v1/tenants does not contain '${TENANT_ID}'" "no ${TENANT_ID}" "$BODY"
fi

# Test: get nonexistent tenant -> 404
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" "${CONTROL_PLANE}/api/v1/tenants/${TENANT_ID}")
if [ "$HTTP_CODE" = "404" ]; then
    pass "GET /api/v1/tenants/${TENANT_ID} returns 404"
else
    fail "GET /api/v1/tenants/${TENANT_ID} returns 404" "404" "$HTTP_CODE"
fi

# ============================================================
# Phase 2: Tenant Creation
# ============================================================
section "Tenant Creation"

CREATE_RESP=$(curl -s -w "\n%{http_code}" -X POST "${CONTROL_PLANE}/api/v1/tenants" \
    -H "Content-Type: application/json" \
    -d "{
        \"tenant_id\": \"${TENANT_ID}\",
        \"tier\": \"pro\",
        \"channels\": [\"web\"],
        \"env_vars\": {
            \"ANTHROPIC_API_KEY\": \"sk-ant-test\"
        }
    }")

HTTP_CODE=$(echo "$CREATE_RESP" | tail -1)
BODY=$(echo "$CREATE_RESP" | sed '$d')

if [ "$HTTP_CODE" = "201" ]; then
    pass "POST /api/v1/tenants returns 201"
else
    fail "POST /api/v1/tenants returns 201" "201" "$HTTP_CODE"
    echo -e "       Body: $BODY"
    cleanup
    exit 1
fi

VM_IP=$(echo "$BODY" | python3 -c "import sys,json; print(json.load(sys.stdin)['vm_ip'])" 2>/dev/null || echo "")
if [ -n "$VM_IP" ]; then
    pass "Tenant created with vm_ip=${VM_IP}"
else
    fail "Tenant has vm_ip" "non-empty" "$VM_IP"
    cleanup
    exit 1
fi

# Ensure cleanup runs on exit from this point
trap cleanup EXIT

# Test: duplicate creation -> error
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" -X POST "${CONTROL_PLANE}/api/v1/tenants" \
    -H "Content-Type: application/json" \
    -d "{\"tenant_id\": \"${TENANT_ID}\", \"tier\": \"pro\", \"channels\": [\"web\"], \"env_vars\": {}}")
if [ "$HTTP_CODE" != "201" ]; then
    pass "Duplicate tenant creation rejected (HTTP ${HTTP_CODE})"
else
    fail "Duplicate tenant creation rejected" "non-201" "$HTTP_CODE"
fi

# Test: tenant appears in list
BODY=$(curl -s "${CONTROL_PLANE}/api/v1/tenants")
if echo "$BODY" | grep -q "\"${TENANT_ID}\""; then
    pass "Tenant appears in GET /api/v1/tenants"
else
    fail "Tenant appears in GET /api/v1/tenants" "${TENANT_ID}" "$BODY"
fi

# ============================================================
# Phase 3: Wait for VM boot
# ============================================================
section "VM Boot"

echo -e "  ${YELLOW}Waiting for VM to boot...${NC}"
MAX_WAIT=30
ELAPSED=0
VM_READY=false
while [ $ELAPSED -lt $MAX_WAIT ]; do
    if curl -s --connect-timeout 2 "http://${VM_IP}:8080/" > /dev/null 2>&1; then
        VM_READY=true
        break
    fi
    sleep 2
    ELAPSED=$((ELAPSED + 2))
done

if [ "$VM_READY" = true ]; then
    pass "VM is reachable at ${VM_IP}:8080 (${ELAPSED}s)"
else
    fail "VM is reachable at ${VM_IP}:8080" "response within ${MAX_WAIT}s" "timeout"
    exit 1
fi

# ============================================================
# Phase 4: Proxy - x-tenant-id header routing
# ============================================================
section "Proxy (x-tenant-id header)"

# Test: GET root page through proxy
BODY=$(curl -s -H "x-tenant-id: ${TENANT_ID}" "${CONTROL_PLANE}/")
if echo "$BODY" | grep -q "MicroClaw Web"; then
    pass "GET / with x-tenant-id returns MicroClaw Web HTML"
else
    fail "GET / with x-tenant-id returns HTML" "contains 'MicroClaw Web'" "$(echo "$BODY" | cut -c1-80)"
fi

# Test: GET static asset through proxy
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" -H "x-tenant-id: ${TENANT_ID}" "${CONTROL_PLANE}/assets/index-DdRpaMK2.js")
if [ "$HTTP_CODE" = "200" ]; then
    pass "GET /assets/*.js returns 200"
else
    # Asset filename may differ; 404 is also acceptable if hash changed
    echo -e "  ${YELLOW}SKIP${NC} GET /assets/*.js returned ${HTTP_CODE} (asset hash may differ)"
fi

# Test: GET /api/sessions through proxy (expect unauthorized or valid JSON)
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" -H "x-tenant-id: ${TENANT_ID}" "${CONTROL_PLANE}/api/sessions")
if [ "$HTTP_CODE" = "200" ] || [ "$HTTP_CODE" = "401" ] || [ "$HTTP_CODE" = "403" ]; then
    pass "GET /api/sessions proxied (HTTP ${HTTP_CODE})"
else
    fail "GET /api/sessions proxied" "200/401/403" "$HTTP_CODE"
fi

# Test: POST /api/send through proxy
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" -X POST \
    -H "x-tenant-id: ${TENANT_ID}" \
    -H "Content-Type: application/json" \
    -d '{"message":"test","session_key":"test"}' \
    "${CONTROL_PLANE}/api/send")
if [ "$HTTP_CODE" = "200" ] || [ "$HTTP_CODE" = "401" ] || [ "$HTTP_CODE" = "403" ] || [ "$HTTP_CODE" = "400" ]; then
    pass "POST /api/send proxied (HTTP ${HTTP_CODE})"
else
    fail "POST /api/send proxied" "200/401/403/400" "$HTTP_CODE"
fi

# Test: query string preserved
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" -H "x-tenant-id: ${TENANT_ID}" "${CONTROL_PLANE}/api/sessions?limit=10")
if [ "$HTTP_CODE" = "200" ] || [ "$HTTP_CODE" = "401" ] || [ "$HTTP_CODE" = "403" ]; then
    pass "Query string preserved through proxy (HTTP ${HTTP_CODE})"
else
    fail "Query string preserved through proxy" "200/401/403" "$HTTP_CODE"
fi

# ============================================================
# Phase 5: Proxy - no header (passthrough to control plane)
# ============================================================
section "Proxy passthrough (no x-tenant-id)"

# Test: /health still works without header
BODY=$(curl -s "${CONTROL_PLANE}/health")
if echo "$BODY" | grep -q '"ok"'; then
    pass "GET /health without header returns control plane health"
else
    fail "GET /health without header" '{"status":"ok"}' "$BODY"
fi

# Test: /api/v1/tenants still works without header
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" "${CONTROL_PLANE}/api/v1/tenants")
if [ "$HTTP_CODE" = "200" ]; then
    pass "GET /api/v1/tenants without header returns 200"
else
    fail "GET /api/v1/tenants without header" "200" "$HTTP_CODE"
fi

# ============================================================
# Phase 6: Proxy - error cases
# ============================================================
section "Proxy error cases"

# Test: nonexistent tenant
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" -H "x-tenant-id: nonexistent" "${CONTROL_PLANE}/")
if [ "$HTTP_CODE" = "404" ]; then
    pass "Nonexistent tenant returns 404"
else
    fail "Nonexistent tenant returns 404" "404" "$HTTP_CODE"
fi

# Test: empty tenant id
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" -H "x-tenant-id: " "${CONTROL_PLANE}/")
if [ "$HTTP_CODE" = "404" ]; then
    pass "Empty x-tenant-id returns 404"
else
    # Empty header might be treated as no header by some implementations
    echo -e "  ${YELLOW}INFO${NC} Empty x-tenant-id returned HTTP ${HTTP_CODE}"
fi

# ============================================================
# Phase 7: Tenant Lifecycle
# ============================================================
section "Tenant Lifecycle"

# Test: stop tenant
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" -X POST "${CONTROL_PLANE}/api/v1/tenants/${TENANT_ID}/stop")
if [ "$HTTP_CODE" = "200" ]; then
    pass "POST /stop returns 200"
else
    fail "POST /stop returns 200" "200" "$HTTP_CODE"
fi

# Test: proxy after stop -> bad gateway (VM is down)
sleep 2
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" --connect-timeout 3 -H "x-tenant-id: ${TENANT_ID}" "${CONTROL_PLANE}/")
if [ "$HTTP_CODE" = "502" ] || [ "$HTTP_CODE" = "504" ]; then
    pass "Proxy to stopped VM returns ${HTTP_CODE}"
else
    # Could also timeout
    echo -e "  ${YELLOW}INFO${NC} Proxy to stopped VM returned HTTP ${HTTP_CODE}"
fi

# Test: delete tenant
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" -X DELETE "${CONTROL_PLANE}/api/v1/tenants/${TENANT_ID}")
if [ "$HTTP_CODE" = "200" ]; then
    pass "DELETE tenant returns 200"
else
    fail "DELETE tenant returns 200" "200" "$HTTP_CODE"
fi

# Test: tenant gone from list
BODY=$(curl -s "${CONTROL_PLANE}/api/v1/tenants")
if ! echo "$BODY" | grep -q "\"${TENANT_ID}\""; then
    pass "Tenant removed from list after delete"
else
    fail "Tenant removed from list after delete" "not in list" "$BODY"
fi

# Disarm the trap since we already cleaned up
trap - EXIT

# ============================================================
# Summary
# ============================================================
echo ""
echo -e "${CYAN}=============================${NC}"
TOTAL=$((PASSED + FAILED))
echo -e "  Total:  ${TOTAL}"
echo -e "  ${GREEN}Passed: ${PASSED}${NC}"
if [ "$FAILED" -gt 0 ]; then
    echo -e "  ${RED}Failed: ${FAILED}${NC}"
    echo -e "${CYAN}=============================${NC}"
    exit 1
else
    echo -e "  ${RED}Failed: ${FAILED}${NC}"
    echo -e "${CYAN}=============================${NC}"
    echo -e "${GREEN}All tests passed!${NC}"
fi
