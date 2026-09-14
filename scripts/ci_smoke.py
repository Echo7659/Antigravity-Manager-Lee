"""验证无账号容器的版本、管理鉴权与前端资源。"""
import argparse
import json
import re
import time
import urllib.error
import urllib.parse
import urllib.request

parser = argparse.ArgumentParser()
parser.add_argument('--base-url', default='http://127.0.0.1:18045/')
parser.add_argument('--version', required=True)
args = parser.parse_args()
base = args.base_url.rstrip('/') + '/'
for attempt in range(40):
    try:
        with urllib.request.urlopen(base + 'health', timeout=5) as response:
            health = json.load(response)
        break
    except (urllib.error.URLError, ConnectionError, TimeoutError):
        if attempt == 39:
            raise
        time.sleep(1)
assert health.get('status') == 'ok' and health.get('version') == args.version, health
request = urllib.request.Request(base + 'api/accounts', headers={'Authorization': 'Bearer ci-smoke-only'})
with urllib.request.urlopen(request, timeout=10) as response:
    assert json.load(response)['accounts'] == []
with urllib.request.urlopen(base, timeout=10) as response:
    html = response.read().decode()
assets = re.findall(r'(?:src|href)="([^"]+\.(?:js|css))"', html)
assert assets, 'No frontend assets found'
for asset in assets:
    with urllib.request.urlopen(urllib.parse.urljoin(base, asset), timeout=10) as response:
        assert response.status == 200 and response.read()
print('Container health, authenticated accounts endpoint and frontend assets passed.')
