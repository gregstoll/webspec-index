import { describe, it, expect, beforeEach } from 'vitest';
import { createTraceStore } from './store';
import type { TraceEntry } from './store';

function fakeStorage(): { getItem(k: string): string | null; setItem(k: string, v: string): void; store: Record<string, string> } {
  const store: Record<string, string> = {};
  return {
    store,
    getItem: (k: string) => store[k] ?? null,
    setItem: (k: string, v: string) => { store[k] = v; },
  };
}

// Entries passed to add() do not include id — the store assigns it.
const A = { spec: 'HTML', anchor: 'navigate' };
const B = { spec: 'HTML', anchor: 'browsing' };
const C = { spec: 'DOM', anchor: 'concept-tree' };

describe('TraceStore', () => {
  let storage: ReturnType<typeof fakeStorage>;

  beforeEach(() => {
    storage = fakeStorage();
  });

  it('starts empty', () => {
    const store = createTraceStore(storage);
    expect(store.entries).toEqual([]);
  });

  it('add appends entry with assigned id', () => {
    const store = createTraceStore(storage);
    store.add(A);
    expect(store.entries).toHaveLength(1);
    expect(store.entries[0]).toMatchObject(A);
    expect(typeof store.entries[0].id).toBe('string');
    expect(store.entries[0].id.length).toBeGreaterThan(0);
  });

  it('add multiple entries in order', () => {
    const store = createTraceStore(storage);
    store.add(A);
    store.add(B);
    store.add(C);
    expect(store.entries.map((e) => e.anchor)).toEqual(['navigate', 'browsing', 'concept-tree']);
  });

  it('remove deletes by index', () => {
    const store = createTraceStore(storage);
    store.add(A);
    store.add(B);
    store.add(C);
    store.remove(1);
    expect(store.entries.map((e) => e.anchor)).toEqual(['navigate', 'concept-tree']);
  });

  it('remove is a no-op for out-of-range index', () => {
    const store = createTraceStore(storage);
    store.add(A);
    store.remove(5);
    expect(store.entries).toHaveLength(1);
  });

  it('move reorders entries', () => {
    const store = createTraceStore(storage);
    store.add(A);
    store.add(B);
    store.add(C);
    store.move(0, 2);
    expect(store.entries.map((e) => e.anchor)).toEqual(['browsing', 'concept-tree', 'navigate']);
  });

  it('move: ids follow entries', () => {
    const store = createTraceStore(storage);
    store.add(A);
    store.add(B);
    store.add(C);
    const ids = store.entries.map((e) => e.id);
    store.move(0, 2);
    expect(store.entries.map((e) => e.id)).toEqual([ids[1], ids[2], ids[0]]);
  });

  it('move is no-op when i === j', () => {
    const store = createTraceStore(storage);
    store.add(A);
    store.add(B);
    store.move(0, 0);
    expect(store.entries.map((e) => e.anchor)).toEqual(['navigate', 'browsing']);
  });

  it('move down by one', () => {
    const store = createTraceStore(storage);
    store.add(A);
    store.add(B);
    store.add(C);
    store.move(0, 1);
    expect(store.entries.map((e) => e.anchor)).toEqual(['browsing', 'navigate', 'concept-tree']);
  });

  it('setNote updates note', () => {
    const store = createTraceStore(storage);
    store.add(A);
    store.setNote(0, 'my note');
    expect(store.entries[0].note).toBe('my note');
  });

  it('setNote with empty string removes note', () => {
    const store = createTraceStore(storage);
    store.add({ ...A, note: 'old' });
    store.setNote(0, '');
    expect(store.entries[0].note).toBeUndefined();
  });

  it('clear removes all entries', () => {
    const store = createTraceStore(storage);
    store.add(A);
    store.add(B);
    store.clear();
    expect(store.entries).toHaveLength(0);
  });

  it('persists to storage', () => {
    const store = createTraceStore(storage);
    store.add(A);
    expect(storage.store['webspec-trace']).toBeDefined();
    const parsed = JSON.parse(storage.store['webspec-trace']) as { v: number; entries: TraceEntry[] };
    expect(parsed.v).toBe(1);
    expect(parsed.entries).toHaveLength(1);
    expect(typeof parsed.entries[0].id).toBe('string');
  });

  it('round-trips through storage', () => {
    const store = createTraceStore(storage);
    store.add(A);
    store.add(B);
    const store2 = createTraceStore(storage);
    expect(store2.entries.map((e) => e.anchor)).toEqual(['navigate', 'browsing']);
  });

  it('round-trip preserves ids', () => {
    const store = createTraceStore(storage);
    store.add(A);
    const id = store.entries[0].id;
    const store2 = createTraceStore(storage);
    expect(store2.entries[0].id).toBe(id);
  });

  it('assigns id to entries loaded from storage without one', () => {
    storage.store['webspec-trace'] = JSON.stringify({
      v: 1,
      entries: [{ spec: 'HTML', anchor: 'navigate' }],
    });
    const store = createTraceStore(storage);
    expect(typeof store.entries[0].id).toBe('string');
    expect(store.entries[0].id.length).toBeGreaterThan(0);
  });

  it('subscribe fires on add', () => {
    const store = createTraceStore(storage);
    const calls: number[] = [];
    store.subscribe((entries) => calls.push(entries.length));
    store.add(A);
    store.add(B);
    expect(calls).toEqual([1, 2]);
  });

  it('unsubscribe stops notifications', () => {
    const store = createTraceStore(storage);
    const calls: number[] = [];
    const unsub = store.subscribe((entries) => calls.push(entries.length));
    store.add(A);
    unsub();
    store.add(B);
    expect(calls).toEqual([1]);
  });
});
