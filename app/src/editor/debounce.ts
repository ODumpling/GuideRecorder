export interface Timers {
  setTimeout(fn: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
}

const realTimers: Timers = {
  setTimeout: (fn, ms) => globalThis.setTimeout(fn, ms),
  clearTimeout: (handle) => globalThis.clearTimeout(handle as ReturnType<typeof setTimeout>),
};

export interface Debouncer {
  /** (Re)starts the wait; `fn` runs `ms` after the last call. */
  schedule(): void;
  cancel(): void;
  /** Runs `fn` now if a call is pending. */
  flush(): void;
  readonly pending: boolean;
}

/** Calls `fn` once `ms` have passed without another `schedule()`. */
export function createDebouncer(fn: () => void, ms: number, timers: Timers = realTimers): Debouncer {
  let handle: unknown = null;
  const cancel = () => {
    if (handle !== null) timers.clearTimeout(handle);
    handle = null;
  };
  return {
    schedule() {
      cancel();
      handle = timers.setTimeout(() => {
        handle = null;
        fn();
      }, ms);
    },
    cancel,
    flush() {
      if (handle === null) return;
      cancel();
      fn();
    },
    get pending() {
      return handle !== null;
    },
  };
}
