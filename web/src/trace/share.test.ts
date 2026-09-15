import { describe, it, expect } from 'vitest';
import { encodeTrace, decodeTrace } from './share';
import type { TraceEntry } from './store';

const threeEntries: TraceEntry[] = [
  { id: 'a', spec: 'HTML', anchor: 'navigate' },
  { id: 'b', spec: 'HTML', anchor: 'fetch', step_path: [3, 1] },
  { id: 'c', spec: 'DOM', anchor: 'concept-tree', note: 'start here' },
];

async function encodeRaw(json: string): Promise<string> {
  const encoded = new TextEncoder().encode(json);
  const cs = new CompressionStream('deflate-raw');
  const writer = cs.writable.getWriter();
  const reader = cs.readable.getReader();
  const writePromise = (async () => {
    await writer.write(encoded);
    await writer.close();
  })();
  const chunks: Uint8Array[] = [];
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    chunks.push(value);
  }
  await writePromise;
  const totalLen = chunks.reduce((n, c) => n + c.length, 0);
  const out = new Uint8Array(totalLen);
  let offset = 0;
  for (const c of chunks) { out.set(c, offset); offset += c.length; }
  let str = '';
  for (const b of out) str += String.fromCharCode(b);
  return btoa(str).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

describe('encodeTrace / decodeTrace', () => {
  it('round-trips three entries (ids dropped, data preserved)', async () => {
    const payload = await encodeTrace(threeEntries);
    const decoded = await decodeTrace(payload);
    expect(decoded).toEqual([
      { spec: 'HTML', anchor: 'navigate' },
      { spec: 'HTML', anchor: 'fetch', step_path: [3, 1] },
      { spec: 'DOM', anchor: 'concept-tree', note: 'start here' },
    ]);
  });

  it('payload for 3 entries is under 200 characters', async () => {
    const payload = await encodeTrace(threeEntries);
    expect(payload.length).toBeLessThan(200);
  });

  it('tampered base64url payload throws with a user-readable error', async () => {
    const payload = await encodeTrace(threeEntries);
    const tampered = payload.slice(0, -5) + 'ZZZZZ';
    await expect(decodeTrace(tampered)).rejects.toThrow(/Invalid trace link/);
  });

  it('unknown version throws with a user-readable error', async () => {
    const b64 = await encodeRaw(JSON.stringify({ v: 2, entries: [] }));
    await expect(decodeTrace(b64)).rejects.toThrow(/Invalid trace link.*unsupported version/);
  });

  it('malformed JSON throws with a user-readable error', async () => {
    const b64 = await encodeRaw('not json {{{');
    await expect(decodeTrace(b64)).rejects.toThrow(/Invalid trace link.*not valid JSON/);
  });

  it('step paths that are not positive integers throw with a user-readable error', async () => {
    for (const step_path of [[0], [-1], [1.5], ['2']]) {
      const b64 = await encodeRaw(
        JSON.stringify({ v: 1, entries: [{ spec: 'HTML', anchor: 'navigate', step_path }] }),
      );
      await expect(decodeTrace(b64)).rejects.toThrow(/Invalid trace link.*positive integers/);
    }
  });
});
