import { createServer } from 'http';
import { createReadStream, statSync } from 'fs';
import { join, extname } from 'path';

const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.wasm': 'application/wasm',
  '.bin': 'application/octet-stream',
  '.json': 'application/json; charset=utf-8',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.ico': 'image/x-icon',
};

function mimeType(filePath) {
  return MIME[extname(filePath)] ?? 'application/octet-stream';
}

function resolveFile(root, pathname) {
  const p = join(root, pathname);
  try {
    statSync(p);
    return p;
  } catch {
    return null;
  }
}

export function startServer(root) {
  return new Promise((resolve) => {
    const server = createServer((req, res) => {
      const pathname = new URL(req.url, 'http://localhost').pathname;
      const indexPath = join(root, 'index.html');

      const filePath = pathname === '/'
        ? indexPath
        : resolveFile(root, pathname);

      if (!filePath) {
        res.writeHead(404);
        res.end('Not found');
        return;
      }

      let stat;
      try {
        stat = statSync(filePath);
      } catch {
        res.writeHead(404);
        res.end('Not found');
        return;
      }

      const fileSize = stat.size;
      const mime = mimeType(filePath);
      const rangeHeader = req.headers['range'];

      if (rangeHeader) {
        const match = /bytes=(\d+)-(\d*)/.exec(rangeHeader);
        if (match) {
          const start = parseInt(match[1], 10);
          const end = match[2] ? parseInt(match[2], 10) : fileSize - 1;
          const chunkSize = end - start + 1;
          res.writeHead(206, {
            'Content-Type': mime,
            'Content-Range': `bytes ${start}-${end}/${fileSize}`,
            'Accept-Ranges': 'bytes',
            'Content-Length': chunkSize,
          });
          createReadStream(filePath, { start, end }).pipe(res);
          return;
        }
      }

      res.writeHead(200, {
        'Content-Type': mime,
        'Accept-Ranges': 'bytes',
        'Content-Length': fileSize,
      });
      createReadStream(filePath).pipe(res);
    });

    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address();
      resolve({
        port,
        close: () => new Promise((r) => server.close(r)),
      });
    });
  });
}
