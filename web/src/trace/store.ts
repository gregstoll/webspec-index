export interface TraceEntry {
  id: string;
  spec: string;
  anchor: string;
  step_path?: number[];
  note?: string;
}

type Listener = (entries: readonly TraceEntry[]) => void;

type StorageLike = Pick<Storage, 'getItem' | 'setItem'>;

const LS_KEY = 'webspec-trace';
const SCHEMA_VERSION = 1;

let _idSeq = 0;

function newId(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID();
  }
  return `trace-${Date.now()}-${++_idSeq}`;
}

function ensureId(e: TraceEntry): TraceEntry {
  return e.id ? e : { ...e, id: newId() };
}

function load(storage: StorageLike): TraceEntry[] {
  try {
    const raw = storage.getItem(LS_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw) as { v: number; entries: unknown };
    if (parsed.v !== SCHEMA_VERSION) return [];
    if (!Array.isArray(parsed.entries)) return [];
    return (parsed.entries as TraceEntry[]).map(ensureId);
  } catch {
    return [];
  }
}

function save(storage: StorageLike, entries: TraceEntry[]): void {
  storage.setItem(LS_KEY, JSON.stringify({ v: SCHEMA_VERSION, entries }));
}

export interface ITraceStore {
  readonly entries: readonly TraceEntry[];
  add(entry: Omit<TraceEntry, 'id'>): void;
  remove(i: number): void;
  move(i: number, j: number): void;
  setNote(i: number, text: string): void;
  clear(): void;
  subscribe(fn: Listener): () => void;
}

export function createTraceStore(storage: StorageLike): ITraceStore {
  let _entries: TraceEntry[] = load(storage);
  const _listeners: Set<Listener> = new Set();

  function notify(): void {
    save(storage, _entries);
    for (const fn of _listeners) fn(_entries);
  }

  return {
    get entries() { return _entries; },

    add(entry: Omit<TraceEntry, 'id'>): void {
      _entries = [..._entries, { ...entry, id: newId() }];
      notify();
    },

    remove(i: number): void {
      _entries = _entries.filter((_, idx) => idx !== i);
      notify();
    },

    move(i: number, j: number): void {
      if (i === j || i < 0 || j < 0 || i >= _entries.length || j >= _entries.length) return;
      const arr = [..._entries];
      const [item] = arr.splice(i, 1);
      arr.splice(j, 0, item);
      _entries = arr;
      notify();
    },

    setNote(i: number, text: string): void {
      _entries = _entries.map((e, idx) =>
        idx === i ? { ...e, note: text || undefined } : e,
      );
      notify();
    },

    clear(): void {
      _entries = [];
      notify();
    },

    subscribe(fn: Listener): () => void {
      _listeners.add(fn);
      return () => _listeners.delete(fn);
    },
  };
}

// Singleton backed by the real localStorage (graceful no-op when unavailable, e.g. SSR/tests).
const _noopStorage: StorageLike = {
  getItem: () => null,
  setItem: () => undefined,
};

export const traceStore: ITraceStore = createTraceStore(
  typeof localStorage !== 'undefined' ? localStorage : _noopStorage,
);
