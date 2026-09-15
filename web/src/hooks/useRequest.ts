import { useState, useEffect, useRef } from 'preact/hooks';
import type { WebspecClient } from '../api/client';
import type { Request, Response } from '../api/types';

export type RequestState<T> =
  | { kind: 'loading' }
  | { kind: 'ok'; value: T }
  | { kind: 'error'; code: string; message: string };

export function useRequest<T extends Response>(
  client: WebspecClient,
  req: Request | null,
  deps: unknown[],
): RequestState<T> {
  const [state, setState] = useState<RequestState<T>>({ kind: 'loading' });
  const gen = useRef(0);

  useEffect(() => {
    if (req === null) {
      setState({ kind: 'loading' });
      return;
    }
    const myGen = ++gen.current;
    setState({ kind: 'loading' });

    client.request(req).then((resp) => {
      if (myGen !== gen.current) return;
      if (resp.type === 'error') {
        setState({ kind: 'error', code: resp.code, message: resp.message });
      } else {
        setState({ kind: 'ok', value: resp as T });
      }
    }).catch((e: unknown) => {
      if (myGen !== gen.current) return;
      setState({ kind: 'error', code: 'internal', message: String(e) });
    });
    // deps is the caller-supplied dependency array; caller is responsible for its contents
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);

  return state;
}
