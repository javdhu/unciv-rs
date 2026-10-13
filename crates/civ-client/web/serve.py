"""Serve the wasm client cross-origin isolated (SharedArrayBuffer needs COOP and COEP), append what the page
reports to POST /stats to stats.jsonl beside this file, and write review shots posted to /shot?name=... into
shots/. Local development only."""
import http.server
import json
import pathlib
import sys
import time
import urllib.parse

ROOT = pathlib.Path(__file__).resolve().parent
PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 5310


class Handler(http.server.SimpleHTTPRequestHandler):
    extensions_map = {**http.server.SimpleHTTPRequestHandler.extensions_map,
                      '.wasm': 'application/wasm', '.js': 'text/javascript'}

    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=str(ROOT), **kwargs)

    def end_headers(self):
        self.send_header('Cross-Origin-Opener-Policy', 'same-origin')
        self.send_header('Cross-Origin-Embedder-Policy', 'require-corp')
        self.send_header('Cache-Control', 'no-store')
        super().end_headers()

    def do_POST(self):
        url = urllib.parse.urlparse(self.path)
        body = self.rfile.read(int(self.headers.get('Content-Length', 0)))
        if url.path == '/shot':
            name = urllib.parse.parse_qs(url.query).get('name', ['shot'])[0]
            safe = ''.join(ch for ch in name if ch.isalnum() or ch in '-_')
            (ROOT / 'shots').mkdir(exist_ok=True)
            (ROOT / 'shots' / f'{safe}.png').write_bytes(body)
            self.send_response(204)
            self.end_headers()
            return
        if url.path != '/stats':
            self.send_error(404)
            return
        record = {'received': time.time(), **json.loads(body)}
        with open(ROOT / 'stats.jsonl', 'a') as f:
            f.write(json.dumps(record) + '\n')
        self.send_response(204)
        self.end_headers()

    def log_message(self, *args):
        pass


http.server.ThreadingHTTPServer(('127.0.0.1', PORT), Handler).serve_forever()
