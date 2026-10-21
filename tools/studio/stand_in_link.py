"""A stand-in for Meshvault's Studio link, on another port, for trying the
served exporters in Studio without touching a running Meshvault.

GET  /export.luau[?schema=N][&refuse=1]  the exporter, sending here (or to /refuse)
GET  /file/<name>                        a file from this folder (e.g. sample_animator.luau)
POST /export                             accepts pieces like Meshvault, saved as export.json
POST /samples/<name>                     Studio pose samples, saved as <name>.json
POST /refuse                             refuses every piece with 413

Usage: python tools/studio/stand_in_link.py [--port 34876] [--out DIR]
Files are saved under --out (default: target/stand-in in the repository)."""
import argparse
import http.server
import os
import urllib.parse

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
EXPORTERS = os.path.join(REPO, 'crates', 'meshvault', 'studio')
# what Meshvault answers when an export arrives, so the exporter's message reads the same
SAVED_REPLY = 'saving it as Place1-123.json in Documents › Meshvault › Exports'

parser = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
parser.add_argument('--port', type=int, default=34876)
parser.add_argument('--out', default=os.path.join(REPO, 'target', 'stand-in'))
args = parser.parse_args()
os.makedirs(args.out, exist_ok=True)

# pieces received so far, by export id
RECEIVED = {}


class Handler(http.server.BaseHTTPRequestHandler):
    def reply(self, code, body):
        body = body.encode()
        self.send_response(code)
        self.send_header('Content-Type', 'text/plain')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        url = urllib.parse.urlparse(self.path)
        print('GET', self.path, flush=True)
        if url.path.startswith('/file/'):
            name = os.path.basename(url.path[len('/file/'):])
            with open(os.path.join(HERE, name), encoding='utf-8') as f:
                return self.reply(200, f.read())
        query = urllib.parse.parse_qs(url.query)
        schema = query.get('schema', ['2'])[0]
        with open(os.path.join(EXPORTERS, f'exporter_v{int(schema)}.luau'), encoding='utf-8') as f:
            source = f.read()
        target = 'refuse' if 'refuse' in query else 'export'
        source = source.replace('__MESHVAULT_URL__', f'http://localhost:{args.port}/{target}')
        source = source.replace('__MESHVAULT_VERSION__', 'stand-in')
        self.reply(200, source)

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get('Content-Length', 0)))
        chunk = self.headers.get('X-Meshvault-Chunk', '1/1')
        export = self.headers.get('X-Meshvault-Export')
        print('POST', self.path, chunk, len(body), flush=True)
        if self.path == '/refuse':
            return self.reply(413, 'piece too large')
        pieces = RECEIVED.setdefault(export, [])
        pieces.append(body)
        i, n = map(int, chunk.split('/'))
        if i == n:
            if self.path.startswith('/samples/'):
                name = os.path.basename(self.path[len('/samples/'):]) + '.json'
            else:
                name = 'export.json'
            path = os.path.join(args.out, name)
            with open(path, 'wb') as f:
                f.write(b''.join(pieces))
            RECEIVED.pop(export, None)
            print('saved', path, flush=True)
        self.reply(200, SAVED_REPLY)

    def log_message(self, *_):
        pass


print(f'stand-in link on http://localhost:{args.port}, saving to {args.out}', flush=True)
http.server.HTTPServer(('127.0.0.1', args.port), Handler).serve_forever()
