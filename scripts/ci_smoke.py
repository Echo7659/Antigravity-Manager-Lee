"""验证无账号容器的版本、管理鉴权、模型目录、删除路由与 Web 资源。"""
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
parser.add_argument('--admin-password', default='ci-smoke-only')
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
for path in ['api/accounts', 'api/proxy/models']:
    try:
        urllib.request.urlopen(base + path, timeout=10).close()
    except urllib.error.HTTPError as error:
        assert error.code == 401, (path, error.code)
    else:
        raise AssertionError(f'{path} accepted an unauthenticated request')

headers = {'Authorization': 'Bearer ' + args.admin_password}
request = urllib.request.Request(base + 'api/accounts', headers=headers)
with urllib.request.urlopen(request, timeout=10) as response:
    assert json.load(response)['accounts'] == []
request = urllib.request.Request(base + 'api/proxy/models', headers=headers)
with urllib.request.urlopen(request, timeout=10) as response:
    models = json.load(response)
assert isinstance(models, list) and all(isinstance(model, str) for model in models), models
request = urllib.request.Request(base + 'api/system/autostart/status', headers=headers)
try:
    urllib.request.urlopen(request, timeout=10).close()
except urllib.error.HTTPError as error:
    assert error.code == 404, error.code
else:
    raise AssertionError('Removed admin route must return 404')
with urllib.request.urlopen(base, timeout=10) as response:
    assert response.status == 200 and 'text/html' in response.headers.get('Content-Type', '')
    html = response.read().decode()
assert '<div id="root">' in html, 'Missing Web app root'
assets = re.findall(r'(?:src|href)="([^"]+\.(?:js|css))"', html)
assert assets, 'No frontend assets found'
for asset in assets:
    with urllib.request.urlopen(urllib.parse.urljoin(base, asset), timeout=10) as response:
        assert response.status == 200 and response.read()
print('Container health/version, admin authentication, model catalog, removed-route 404 and Web assets passed.')
