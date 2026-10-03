"""验证容器 smoke 会拒绝鉴权、模型目录和删除路由的回归。"""
import http.server
import json
import pathlib
import subprocess
import sys
import threading
import unittest

SMOKE_PROGRAM = [sys.executable, str(pathlib.Path(__file__).with_name('ci_smoke.py'))]


class SmokeContractTests(unittest.TestCase):
    def run_smoke(self, regression=None):
        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def do_GET(self):
                authenticated = self.headers.get('Authorization') == 'Bearer ci-smoke-only'
                status, content_type = 200, 'application/json'
                if self.path == '/health':
                    payload = {'status': 'ok', 'version': '4.9.1'}
                elif self.path in ('/api/accounts', '/api/proxy/models'):
                    if not authenticated and regression != 'open-admin':
                        status, payload = 401, {}
                    elif self.path == '/api/accounts':
                        payload = {'accounts': []}
                    else:
                        payload = {} if regression == 'bad-models' else ['fixture-model']
                elif self.path == '/api/system/autostart/status':
                    status, payload = (200 if regression == 'removed-route' else 404), {}
                elif self.path == '/':
                    content_type = 'text/html'
                    payload = '<div id="root"></div><script src="/assets/app.js"></script>'
                elif self.path == '/assets/app.js':
                    content_type, payload = 'text/javascript', 'window.fixture = true;'
                else:
                    status, payload = 404, {}
                body = (payload if isinstance(payload, str) else json.dumps(payload)).encode()
                self.send_response(status)
                self.send_header('Content-Type', content_type)
                self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                self.wfile.write(body)

        with http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler) as server:
            thread = threading.Thread(target=server.serve_forever)
            thread.start()
            try:
                return subprocess.run(
                    SMOKE_PROGRAM + ['--base-url', f'http://127.0.0.1:{server.server_port}', '--version', '4.9.1'],
                    capture_output=True, text=True, timeout=20,
                )
            finally:
                server.shutdown()
                thread.join(timeout=5)

    def test_complete_contract_passes(self):
        result = self.run_smoke()
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_unprotected_admin_is_rejected(self):
        self.assertNotEqual(self.run_smoke('open-admin').returncode, 0)

    def test_invalid_catalog_is_rejected(self):
        self.assertNotEqual(self.run_smoke('bad-models').returncode, 0)

    def test_removed_route_is_rejected(self):
        self.assertNotEqual(self.run_smoke('removed-route').returncode, 0)


if __name__ == '__main__':
    unittest.main()
