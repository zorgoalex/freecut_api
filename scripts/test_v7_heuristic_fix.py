import json, urllib.request
# Quick verify: optimize_valid should now return 3 placements (not 6)
with open('tests/fixtures/optimize_valid.json') as f:
    req = json.load(f)
req['params']['seed'] = 1
req['params']['retry_strategy'] = 'disabled'
req['params']['time_limit_ms'] = 1000
req['params']['restarts'] = 1
payload = json.dumps(req).encode()
r = urllib.request.Request('http://localhost:8088/v1/optimize', data=payload, headers={'Content-Type': 'application/json'})
resp = urllib.request.urlopen(r, timeout=30)
data = json.loads(resp.read())
total = sum(len(s['placements']) for s in data['solutions'])
print('V7 fix verification: optimize_valid has {} placements (expected 3)'.format(total))
for s in data['solutions']:
    for p in s['placements']:
        print('  {} inst={}'.format(p['item_id'], p['instance']))
