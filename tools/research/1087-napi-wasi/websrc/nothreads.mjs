// Issue #1087: a hand-written loader for the *non-threaded* napi WASI build
// (wasm32-wasip1, emnapi "basic"), which napi-rs's CLI does not generate a loader for.
// Plain (unshared) memory, no Worker pool, so no cross-origin isolation needed.
import { instantiateNapiModule, WASI, emnapiAsyncWorkPlugin, emnapiTSFNPlugin } from '@napi-rs/wasm-runtime';
import { getDefaultContext } from '@emnapi/runtime';

export async function load(wasmBytes) {
  const memory = new WebAssembly.Memory({ initial: 4000, maximum: 65536 });
  const { napiModule } = await instantiateNapiModule(wasmBytes, {
    context: getDefaultContext(),
    asyncWorkPoolSize: 0,
    wasi: new WASI({ version: 'preview1' }),
    plugins: [emnapiAsyncWorkPlugin, emnapiTSFNPlugin],
    overwriteImports(importObject) {
      importObject.env = { ...importObject.env, ...importObject.napi, ...importObject.emnapi, memory };
      return importObject;
    },
    beforeInit({ instance }) {
      for (const name of Object.keys(instance.exports)) {
        if (name.startsWith('__napi_register__')) instance.exports[name]();
      }
    },
  });
  return napiModule.exports;
}
