import type { Request, Response } from './types';
import type { WebspecClient } from './client';

export interface Stats { fetches: number; bytes_fetched: number; cache_hits: number }

export interface WasmModule {
  default(): Promise<unknown>;
  open(manifestUrl: string): void;
  handle(requestJson: string): string;
  stats(): string;
}

export class WasmClient implements WebspecClient {
  private initialised: WasmModule | undefined;
  private module: WasmModule | undefined;
  private opening: Promise<WasmModule> | undefined;

  constructor(private readonly load: () => Promise<WasmModule>, private readonly manifestUrl: string) {}

  private ready(): Promise<WasmModule> {
    if (this.module) return Promise.resolve(this.module);
    if (!this.opening) {
      this.opening = (async () => {
        const mod = this.initialised ?? await (async () => {
          const m = await this.load();
          await m.default();
          return m;
        })();
        this.initialised = mod;
        mod.open(this.manifestUrl);
        this.module = mod;
        return mod;
      })().finally(() => { this.opening = undefined; });
    }
    return this.opening;
  }

  async request(req: Request): Promise<Response> {
    const mod = await this.ready();
    return JSON.parse(mod.handle(JSON.stringify(req))) as Response;
  }

  async stats(): Promise<Stats> {
    const mod = await this.ready();
    return JSON.parse(mod.stats()) as Stats;
  }
}
