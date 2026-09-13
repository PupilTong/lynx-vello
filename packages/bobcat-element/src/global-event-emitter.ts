// Lynx's JS GlobalEventEmitter is an argument-list bus, separate from
// ContextProxy and DOM EventTarget. Native reference:
// lynx/js_libraries/lynx-core/src/modules/event/eventEmitter.ts.
export class GlobalEventEmitter {
  #events = new Map<string, {listener: (...args: unknown[]) => void; context: object | undefined}[]>();

  addListener(name: string, listener: (...args: unknown[]) => void, context?: object) {
    const listeners = this.#events.get(name);
    const registration = { listener, context };
    if (listeners) listeners.push(registration);
    else this.#events.set(name, [registration]);
  }

  removeListener(name: string, listener: (...args: unknown[]) => void) {
    if (typeof listener !== "function") {
      throw new Error("removeListener only takes instances of Function");
    }
    const listeners = this.#events.get(name);
    const index = listeners?.findIndex(item => item.listener === listener) ?? -1;
    if (listeners && index !== -1) listeners.splice(index, 1);
  }

  emit(name: string, args: unknown[]) {
    // Native deliberately iterates the live array: splice during a listener
    // affects the remaining walk. A thrown listener ends this emission.
    this.#events.get(name)?.forEach(({ listener, context }) => {
      if (typeof listener === "function") listener.apply(context || this, args);
    });
  }

  trigger(name: string, payload: unknown) {
    const listeners = this.#events.get(name);
    if (listeners) {
      if (typeof payload === "string") payload = JSON.parse(payload);
      listeners.forEach(({ listener, context }) => {
        if (typeof listener === "function") listener.call(context || this, payload);
      });
    }
  }

  removeAllListeners(name?: string) {
    if (typeof name === "string") this.#events.delete(name);
    else this.#events = new Map();
  }

  toggle(name: string, ...args: unknown[]) {
    this.emit(name, args);
  }
}
