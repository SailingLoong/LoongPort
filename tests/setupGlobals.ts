import { webcrypto } from "node:crypto";
import {
  ReadableStream,
  TransformStream,
  WritableStream,
} from "node:stream/web";

// Polyfill ResizeObserver for jsdom/happy-dom
if (typeof globalThis.ResizeObserver === "undefined") {
  globalThis.ResizeObserver = class ResizeObserver {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof globalThis.ResizeObserver;
}

// vm 系池（pool: vmThreads）的 VM 上下文不继承 Node 的 Web Streams / WebCrypto
// 全局（msw/node、crypto.subtle 首当其冲；jsdom 的 crypto 没有 subtle）；
// 普通 forks/threads 池自带这些全局，此处在那里是 no-op。
if (typeof globalThis.TransformStream === "undefined") {
  Object.assign(globalThis, {
    TransformStream,
    ReadableStream,
    WritableStream,
  });
}
if (typeof globalThis.crypto?.subtle === "undefined") {
  Object.defineProperty(globalThis, "crypto", {
    value: webcrypto,
    configurable: true,
  });
}

const storage = new Map<string, string>();

if (
  typeof globalThis.localStorage === "undefined" ||
  typeof globalThis.localStorage?.getItem !== "function"
) {
  Object.defineProperty(globalThis, "localStorage", {
    value: {
      getItem: (key: string) => storage.get(key) ?? null,
      setItem: (key: string, value: string) => {
        storage.set(key, String(value));
      },
      removeItem: (key: string) => {
        storage.delete(key);
      },
      clear: () => {
        storage.clear();
      },
      key: (index: number) => Array.from(storage.keys())[index] ?? null,
      get length() {
        return storage.size;
      },
    },
    configurable: true,
  });
}

if (!Element.prototype.hasPointerCapture) {
  Element.prototype.hasPointerCapture = () => false;
}

if (!Element.prototype.setPointerCapture) {
  Element.prototype.setPointerCapture = () => {};
}

if (!Element.prototype.releasePointerCapture) {
  Element.prototype.releasePointerCapture = () => {};
}
