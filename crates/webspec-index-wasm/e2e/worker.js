import init, { open, handle, stats } from "./pkg/webspec_index_wasm.js";

const ready = init();

self.onmessage = async (e) => {
  await ready;
  const { type, id } = e.data;
  try {
    let result;
    if (type === "open") {
      open(e.data.manifestUrl);
      result = { ok: true };
    } else if (type === "handle") {
      result = handle(e.data.request);
    } else if (type === "stats") {
      result = stats();
    }
    self.postMessage({ id, ok: true, result });
  } catch (err) {
    self.postMessage({ id, ok: false, error: String(err) });
  }
};
