#!/usr/bin/env bash
# consul-lookup.sh — Look up a service by name
# Usage: consul-lookup.sh <service-name> [format]
# Format: address (default), url, json, all
set -euo pipefail

SERVICE="${1:?Usage: consul-lookup.sh <service-name> [address|url|json|all]}"
FORMAT="${2:-address}"
CONSUL_HTTP_ADDR="${CONSUL_HTTP_ADDR:-http://127.0.0.1:8500}"

# Query the service
RESULT=$(curl -sf "${CONSUL_HTTP_ADDR}/v1/catalog/service/${SERVICE}" 2>/dev/null)

if [ -z "$RESULT" ] || [ "$RESULT" = "[]" ]; then
    echo "ERROR: Service '$SERVICE' not found" >&2
    exit 1
fi

case "$FORMAT" in
    address)
        echo "$RESULT" | python3 -c "
import json,sys
svc = json.load(sys.stdin)
for s in svc:
    addr = s.get('ServiceAddress') or s.get('Address')
    port = s.get('ServicePort')
    print(f'{addr}:{port}')
"
        ;;
    url)
        echo "$RESULT" | python3 -c "
import json,sys
svc = json.load(sys.stdin)
for s in svc:
    addr = s.get('ServiceAddress') or s.get('Address')
    port = s.get('ServicePort')
    print(f'http://{addr}:{port}')
"
        ;;
    json)
        echo "$RESULT" | python3 -m json.tool
        ;;
    all)
        echo "$RESULT" | python3 -c "
import json,sys
svc = json.load(sys.stdin)
for s in svc:
    addr = s.get('ServiceAddress') or s.get('Address')
    port = s.get('ServicePort')
    tags = ','.join(s.get('ServiceTags', []))
    meta = s.get('ServiceMeta', {})
    node = s.get('Node')
    print(f'{s[\"ServiceName\"]}  {addr}:{port}  node={node}  tags=[{tags}]')
    for k,v in meta.items():
        print(f'  {k}={v}')
"
        ;;
    *)
        echo "Unknown format: $FORMAT" >&2
        echo "Options: address, url, json, all" >&2
        exit 1
        ;;
esac
