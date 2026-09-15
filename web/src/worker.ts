import { MockClient } from './api/client';
import type { WebspecClient } from './api/client';
import type { Request } from './api/types';

type InboundMessage = { id: string; request: Request };

// Replace the return value of loadBackend() with the wasm client once
// webspec-index-wasm is built and the HTTP-range VFS is initialised.
function loadBackend(): WebspecClient {
  return new MockClient();
}

const client = loadBackend();

self.addEventListener('message', (event: MessageEvent<InboundMessage>) => {
  const { id, request } = event.data;
  client.request(request).then(
    (result) => self.postMessage({ id, ok: true, result }),
    (err: unknown) =>
      self.postMessage({
        id,
        ok: false,
        error: { type: 'error', code: 'worker_error', message: String(err) },
      }),
  );
});
