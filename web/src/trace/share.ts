import type { TraceEntry } from './store';

type WireEntry = { spec: string; anchor: string; step_path?: number[]; note?: string };
type Payload = { v: 1; entries: WireEntry[] };

function toBase64url(bytes: Uint8Array): string {
  let str = '';
  for (const b of bytes) str += String.fromCharCode(b);
  return btoa(str).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

function fromBase64url(s: string): Uint8Array {
  const padded = s.replace(/-/g, '+').replace(/_/g, '/');
  const remainder = padded.length % 4;
  const b64 = remainder ? padded + '='.repeat(4 - remainder) : padded;
  const str = atob(b64);
  const buf = new Uint8Array(str.length);
  for (let i = 0; i < str.length; i++) buf[i] = str.charCodeAt(i);
  return buf;
}

async function streamTransform(
  input: Uint8Array,
  transform: { writable: WritableStream<BufferSource>; readable: ReadableStream<Uint8Array> },
): Promise<Uint8Array> {
  const writer = transform.writable.getWriter();
  const reader = transform.readable.getReader();
  const writePromise = (async () => {
    await writer.write(input as unknown as BufferSource);
    await writer.close();
  })();
  // Attach a no-op rejection handler so Node does not emit an unhandled-rejection
  // event if the read loop throws before we reach `await writePromise` below.
  writePromise.catch(() => {});
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
  for (const chunk of chunks) { out.set(chunk, offset); offset += chunk.length; }
  return out;
}

export async function encodeTrace(entries: readonly TraceEntry[]): Promise<string> {
  const wire: WireEntry[] = entries.map(({ spec, anchor, step_path, note }) => {
    const e: WireEntry = { spec, anchor };
    if (step_path && step_path.length > 0) e.step_path = step_path;
    if (note) e.note = note;
    return e;
  });
  const payload: Payload = { v: 1, entries: wire };
  const json = JSON.stringify(payload);
  const encoded = new TextEncoder().encode(json);
  const compressed = await streamTransform(encoded, new CompressionStream('deflate-raw'));
  return toBase64url(compressed);
}

export async function decodeTrace(s: string): Promise<Omit<TraceEntry, 'id'>[]> {
  let raw: Uint8Array;
  try {
    raw = fromBase64url(s);
  } catch {
    throw new Error('Invalid trace link: malformed base64url data.');
  }
  let json: string;
  try {
    const decompressed = await streamTransform(raw, new DecompressionStream('deflate-raw'));
    json = new TextDecoder().decode(decompressed);
  } catch {
    throw new Error('Invalid trace link: data could not be decompressed.');
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(json);
  } catch {
    throw new Error('Invalid trace link: payload is not valid JSON.');
  }
  if (
    typeof parsed !== 'object' ||
    parsed === null ||
    (parsed as Record<string, unknown>)['v'] !== 1 ||
    !Array.isArray((parsed as Record<string, unknown>)['entries'])
  ) {
    const v = (parsed as Record<string, unknown>)?.['v'];
    if (v !== undefined && v !== 1) {
      throw new Error(`Invalid trace link: unsupported version ${String(v)}.`);
    }
    throw new Error('Invalid trace link: malformed payload.');
  }
  const { entries } = parsed as Payload;
  return entries.map((e: unknown, i: number): Omit<TraceEntry, 'id'> => {
    if (typeof e !== 'object' || e === null) {
      throw new Error(`Invalid trace link: entry ${i} is not an object.`);
    }
    const entry = e as Record<string, unknown>;
    if (typeof entry['spec'] !== 'string' || typeof entry['anchor'] !== 'string') {
      throw new Error(`Invalid trace link: entry ${i} missing spec or anchor.`);
    }
    const result: Omit<TraceEntry, 'id'> = {
      spec: entry['spec'],
      anchor: entry['anchor'],
    };
    if (Array.isArray(entry['step_path']) && entry['step_path'].length > 0) {
      const path = entry['step_path'] as unknown[];
      if (!path.every((n) => typeof n === 'number' && Number.isInteger(n) && n > 0)) {
        throw new Error(`Invalid trace link: entry ${i} step path must be positive integers.`);
      }
      result.step_path = path as number[];
    }
    if (typeof entry['note'] === 'string') result.note = entry['note'];
    return result;
  });
}
