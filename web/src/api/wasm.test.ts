import { describe, expect, it, vi } from 'vitest';
import { WasmClient, type WasmModule } from './wasm';

function fakeModule(handle: (json: string) => string): WasmModule & { opens: string[] } {
  const opens: string[] = [];
  return {
    opens,
    default: vi.fn(async () => undefined),
    open: (url) => { opens.push(url); },
    handle,
    stats: () => JSON.stringify({ fetches: 3, bytes_fetched: 196608, cache_hits: 1 }),
  };
}

describe('WasmClient', () => {
  it('initialises and opens once, then dispatches requests as JSON', async () => {
    const mod = fakeModule((json) => {
      const req = JSON.parse(json);
      expect(req).toEqual({ type: 'specs' });
      return JSON.stringify({ type: 'specs', result: { specs: [] } });
    });
    const client = new WasmClient(async () => mod, 'https://example.test/db/manifest.json');
    const [a, b] = await Promise.all([client.request({ type: 'specs' }), client.request({ type: 'specs' })]);
    expect(a).toEqual({ type: 'specs', result: { specs: [] } });
    expect(b).toEqual(a);
    expect(mod.opens).toEqual(['https://example.test/db/manifest.json']);
    expect(mod.default).toHaveBeenCalledTimes(1);
  });

  it('returns error envelopes as resolved responses, not rejections', async () => {
    const mod = fakeModule(() => JSON.stringify({ type: 'error', code: 'not_found', message: 'HTML#nope' }));
    const client = new WasmClient(async () => mod, 'm.json');
    await expect(client.request({ type: 'query', target: 'HTML#nope' })).resolves.toEqual({ type: 'error', code: 'not_found', message: 'HTML#nope' });
  });

  it('rejects when open throws and retries open on the next request', async () => {
    let fail = true;
    const mod = fakeModule(() => JSON.stringify({ type: 'specs', result: { specs: [] } }));
    mod.open = () => { if (fail) throw new Error('HTTP 404'); };
    const client = new WasmClient(async () => mod, 'm.json');
    await expect(client.request({ type: 'specs' })).rejects.toThrow('HTTP 404');
    fail = false;
    await expect(client.request({ type: 'specs' })).resolves.toMatchObject({ type: 'specs' });
  });

  it('parses stats', async () => {
    const client = new WasmClient(async () => fakeModule(() => '{}'), 'm.json');
    await client.request({ type: 'specs' }).catch(() => undefined);
    expect(await client.stats()).toEqual({ fetches: 3, bytes_fetched: 196608, cache_hits: 1 });
  });
});
