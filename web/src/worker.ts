import { MockClient } from './api/client';
import { WasmClient } from './api/wasm';
import type { WebspecClient } from './api/client';
import type { Request } from './api/types';

type InboundMessage = { id: string; request: Request };

function loadBackend(): WebspecClient {
  if (import.meta.env.VITE_BACKEND === 'mock') return new MockClient();
  return new WasmClient(() => import('./wasm/webspec_index_wasm.js'), self.name);
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
