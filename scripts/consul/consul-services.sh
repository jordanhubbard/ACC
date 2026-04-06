#!/usr/bin/env bash
# consul-services.sh — List all registered services with health status
# Usage: consul-services.sh [--healthy | --critical | --all]
set -euo pipefail

CONSUL_HTTP_ADDR="${CONSUL_HTTP_ADDR:-http://127.0.0.1:8500}"
FILTER="${1:---all}"

echo "=== CCC Fleet Services ==="
echo ""

# Get all services
SERVICES=$(curl -sf "${CONSUL_HTTP_ADDR}/v1/catalog/services" 2>/dev/null)
if [ -z "$SERVICES" ] || [ "$SERVICES" = "{}" ]; then
    echo "No services registered (is Consul running?)"
    exit 1
fi

echo "$SERVICES" | python3 -c "
import json, sys, urllib.request

consul = '${CONSUL_HTTP_ADDR}'
services = json.load(sys.stdin)
filter_mode = '${FILTER}'

for name, tags in sorted(services.items()):
    if name == 'consul':
        continue
    try:
        with urllib.request.urlopen(f'{consul}/v1/health/checks/{name}', timeout=2) as resp:
            checks = json.loads(resp.read())
    except:
        checks = []

    status = '✅'
    for c in checks:
        if c['Status'] == 'critical':
            status = '❌'
            break
        elif c['Status'] == 'warning':
            status = '⚠️'

    if filter_mode == '--healthy' and status != '✅':
        continue
    if filter_mode == '--critical' and status != '❌':
        continue

    try:
        with urllib.request.urlopen(f'{consul}/v1/catalog/service/{name}', timeout=2) as resp:
            instances = json.loads(resp.read())
    except:
        instances = []

    for inst in instances:
        addr = inst.get('ServiceAddress') or inst.get('Address')
        port = inst.get('ServicePort', '')
        node = inst.get('Node', '?')
        tag_str = ', '.join(inst.get('ServiceTags', []))
        print(f'  {status} {name:20s} {addr}:{port:<6} node={node:<12} [{tag_str}]')
"

echo ""
echo "Total: $(echo "$SERVICES" | python3 -c "import json,sys; print(len([k for k in json.load(sys.stdin) if k != 'consul']))")"
