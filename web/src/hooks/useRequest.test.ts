// @vitest-environment jsdom
import { describe, it, expect } from 'vitest';
import { renderHook, act } from '@testing-library/preact';
import { useRequest } from './useRequest';
import type { WebspecClient } from '../api/client';
import type { Response } from '../api/types';

type Deferred<T> = { promise: Promise<T>; resolve: (v: T) => void; reject: (e: unknown) => void };
function deferred<T>(): Deferred<T> {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

class ControlledClient implements WebspecClient {
  private queue: Array<Deferred<Response>> = [];

  request(_req: Parameters<WebspecClient['request']>[0]): Promise<Response> {
    const d = deferred<Response>();
    this.queue.push(d);
    return d.promise;
  }

  shift(): Deferred<Response> {
    return this.queue.shift()!;
  }
}

describe('useRequest', () => {
  it('starts as loading', () => {
    const client = new ControlledClient();
    const { result } = renderHook(() =>
      useRequest(client, { type: 'specs' }, []),
    );
    expect(result.current.kind).toBe('loading');
  });

  it('transitions loading → ok on successful response', async () => {
    const client = new ControlledClient();
    const { result } = renderHook(() =>
      useRequest(client, { type: 'specs' }, []),
    );
    expect(result.current.kind).toBe('loading');

    const d = client.shift();
    await act(async () => {
      d.resolve({ type: 'specs', result: { specs: [] } });
      await d.promise;
    });

    expect(result.current.kind).toBe('ok');
    if (result.current.kind === 'ok') {
      expect(result.current.value.type).toBe('specs');
    }
  });

  it('transitions loading → error when response is an error envelope', async () => {
    const client = new ControlledClient();
    const { result } = renderHook(() =>
      useRequest(client, { type: 'specs' }, []),
    );

    const d = client.shift();
    await act(async () => {
      d.resolve({ type: 'error', code: 'not_found', message: 'gone' });
      await d.promise;
    });

    expect(result.current.kind).toBe('error');
    if (result.current.kind === 'error') {
      expect(result.current.code).toBe('not_found');
      expect(result.current.message).toBe('gone');
    }
  });

  it('transitions loading → error on promise rejection with code=internal', async () => {
    const client = new ControlledClient();
    const { result } = renderHook(() =>
      useRequest(client, { type: 'specs' }, []),
    );

    const d = client.shift();
    await act(async () => {
      d.reject(new Error('network failure'));
      await d.promise.catch(() => {});
    });

    expect(result.current.kind).toBe('error');
    if (result.current.kind === 'error') {
      expect(result.current.code).toBe('internal');
      expect(result.current.message).toContain('network failure');
    }
  });

  it('ignores stale responses when deps change before first resolves', async () => {
    const client = new ControlledClient();
    let spec = 'HTML';
    const { result, rerender } = renderHook(() =>
      useRequest(client, { type: 'list', spec }, [spec]),
    );

    const d1 = client.shift();

    // Change deps — triggers a second request.
    spec = 'DOM';
    rerender();

    const d2 = client.shift();

    // Resolve request 1 (now stale) first.
    await act(async () => {
      d1.resolve({ type: 'list', result: { spec: 'HTML', entries: [] } });
      await d1.promise;
    });

    // State should be loading (or error) for the second request, not ok with HTML.
    // The stale result must not win.
    expect(result.current.kind).not.toBe('ok');

    // Resolve request 2.
    await act(async () => {
      d2.resolve({ type: 'list', result: { spec: 'DOM', entries: [] } });
      await d2.promise;
    });

    expect(result.current.kind).toBe('ok');
    if (result.current.kind === 'ok') {
      expect((result.current.value as { type: 'list'; result: { spec: string; entries: unknown[] } }).result.spec).toBe('DOM');
    }
  });
});
