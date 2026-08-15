import {
  finishBrowserScriptCheckpoint,
  initSync,
  wasm_thread_entry_point,
} from './pkg/bobcat_wasm.js'

self.addEventListener(
  'message',
  (event) => {
    const [module, memory, work] = event.data
    try {
      initSync({ module, memory })

      // wasm_thread posts opaque numeric messages. With the pinned
      // spawn_from_worker implementation, nested spawns are created directly
      // and the entry point's only outgoing message is ThreadComplete. Hold it
      // for one task, so Promise microtasks can use retained PAPI closures.
      // The completion timer releases those closures immediately before it
      // forwards ThreadComplete and closes: a FinalizationRegistry cleanup
      // can therefore run only while the closures are still live.
      const nativePostMessage = self.postMessage.bind(self)
      const outgoing = []
      self.postMessage = (...arguments_) => {
        outgoing.push(arguments_)
      }
      wasm_thread_entry_point(work)
      self.postMessage = nativePostMessage

      if (outgoing.length !== 1) {
        throw new Error(
          `wasm_thread entry point posted ${String(outgoing.length)} messages; expected only ThreadComplete`,
        )
      }
      const [completion] = outgoing
      setTimeout(() => {
        finishBrowserScriptCheckpoint()
        nativePostMessage(...completion)
        self.close()
      }, 0)
    } catch (error) {
      console.error(error)
      throw error
    }
  },
  { once: true },
)
