import { initSync, wasm_thread_entry_point } from '../pkg/bobcat_wasm.js'

declare const self: DedicatedWorkerGlobalScope

self.addEventListener(
  'message',
  (event) => {
    const [module, memory, work] = event.data as [WebAssembly.Module, WebAssembly.Memory, number]
    try {
      initSync({ module, memory })
      wasm_thread_entry_point(work)
      self.close()
    } catch (error) {
      console.error(error)
      throw error
    }
  },
  { once: true },
)
