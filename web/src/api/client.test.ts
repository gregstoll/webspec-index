import { describe, it, expect, vi } from 'vitest';
import { WorkerClient, MockClient } from './client';
import type { Response } from './types';

// Minimal fake Worker for unit tests. Synchronously delivers canned responses
// via replyOk/replyError, and can fire an error event via fireError().
class FakeWorker extends EventTarget {
  sentMessages: Array<{ id: string; request: unknown }> = [];
  private messageListeners: Array<(ev: MessageEvent) => void> = [];
  private errorListeners: Array<(ev: ErrorEvent) => void> = [];

  addEventListener(type: string, listener: EventListenerOrEventListenerObject | null): void {
    if (listener && typeof listener === 'function') {
      if (type === 'message') {
        this.messageListeners.push(listener as (ev: MessageEvent) => void);
      } else if (type === 'error') {
        this.errorListeners.push(listener as (ev: ErrorEvent) => void);
      }
    }
    super.addEventListener(type, listener);
  }

  postMessage(data: { id: string; request: unknown }): void {
    this.sentMessages.push(data);
  }

  replyOk(id: string, result: Response): void {
    const event = new MessageEvent('message', { data: { id, ok: true, result } });
    for (const listener of this.messageListeners) listener(event);
  }

  replyError(id: string, error: unknown): void {
    const event = new MessageEvent('message', { data: { id, ok: false, error } });
    for (const listener of this.messageListeners) listener(event);
  }

  fireError(): void {
    const event = new Event('error') as ErrorEvent;
    for (const listener of this.errorListeners) listener(event);
  }
}

describe('WorkerClient', () => {
  it('sends the request with an id and resolves with the response', async () => {
    const worker = new FakeWorker();
    const client = new WorkerClient(worker as unknown as Worker);

    const promise = client.request({ type: 'specs' });
    expect(worker.sentMessages).toHaveLength(1);
    const { id } = worker.sentMessages[0];

    const mockResponse: Response = { type: 'specs', result: { specs: [] } };
    worker.replyOk(id, mockResponse);

    expect(await promise).toEqual(mockResponse);
  });

  it('correlates multiple concurrent requests by id', async () => {
    const worker = new FakeWorker();
    const client = new WorkerClient(worker as unknown as Worker);

    const p1 = client.request({ type: 'specs' });
    const p2 = client.request({ type: 'list', spec: 'HTML' });

    expect(worker.sentMessages).toHaveLength(2);
    const [m1, m2] = worker.sentMessages;

    const r2: Response = { type: 'list', result: { spec: 'HTML', entries: [] } };
    const r1: Response = { type: 'specs', result: { specs: [] } };

    // Reply out of order — p2 first.
    worker.replyOk(m2.id, r2);
    worker.replyOk(m1.id, r1);

    expect(await p1).toEqual(r1);
    expect(await p2).toEqual(r2);
  });

  it('rejects the promise when the worker replies with ok: false', async () => {
    const worker = new FakeWorker();
    const client = new WorkerClient(worker as unknown as Worker);

    const promise = client.request({ type: 'specs' });
    const { id } = worker.sentMessages[0];
    worker.replyError(id, { type: 'error', code: 'oops', message: 'boom' });

    await expect(promise).rejects.toBeDefined();
  });

  it('ignores a response for an unknown id', () => {
    const worker = new FakeWorker();
    new WorkerClient(worker as unknown as Worker);
    expect(() =>
      worker.replyOk('nonexistent', { type: 'specs', result: { specs: [] } }),
    ).not.toThrow();
  });

  it('rejects all pending promises when the worker fires an error event', async () => {
    const worker = new FakeWorker();
    const client = new WorkerClient(worker as unknown as Worker);

    const p1 = client.request({ type: 'specs' });
    const p2 = client.request({ type: 'list', spec: 'HTML' });

    worker.fireError();

    await expect(p1).rejects.toBeInstanceOf(Error);
    await expect(p2).rejects.toBeInstanceOf(Error);
  });
});

describe('MockClient', () => {
  it('returns specs list', async () => {
    const client = new MockClient();
    const resp = await client.request({ type: 'specs' });
    expect(resp.type).toBe('specs');
    if (resp.type === 'specs') {
      expect(resp.result.specs.length).toBeGreaterThan(0);
    }
  });

  it('returns HTML#navigate fixture', async () => {
    const client = new MockClient();
    const resp = await client.request({ type: 'query', target: 'HTML#navigate' });
    expect(resp.type).toBe('query');
    if (resp.type === 'query') {
      expect(resp.result.spec).toBe('HTML');
      expect(resp.result.anchor).toBe('navigate');
      expect(resp.result.type).toBe('algorithm');
    }
  });

  it('returns list fixture with spec field', async () => {
    const client = new MockClient();
    const resp = await client.request({ type: 'list', spec: 'HTML' });
    expect(resp.type).toBe('list');
    if (resp.type === 'list') {
      expect(resp.result.spec).toBe('HTML');
      expect(resp.result.entries.length).toBeGreaterThan(0);
    }
  });

  it('returns error for unknown target', async () => {
    const client = new MockClient();
    const resp = await client.request({ type: 'query', target: 'UNKNOWN#foo' });
    expect(resp.type).toBe('error');
  });

  it('spy confirms request is called', async () => {
    const client = new MockClient();
    const spy = vi.spyOn(client, 'request');
    await client.request({ type: 'specs' });
    expect(spy).toHaveBeenCalledOnce();
  });
});
