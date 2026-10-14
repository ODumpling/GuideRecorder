import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createDebouncer } from "@/editor/debounce";

describe("createDebouncer", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("runs once, the given time after the last schedule", () => {
    const fn = vi.fn();
    const d = createDebouncer(fn, 500);
    d.schedule();
    vi.advanceTimersByTime(400);
    d.schedule();
    vi.advanceTimersByTime(400);
    expect(fn).not.toHaveBeenCalled();
    expect(d.pending).toBe(true);
    vi.advanceTimersByTime(100);
    expect(fn).toHaveBeenCalledTimes(1);
    expect(d.pending).toBe(false);
  });

  it("cancel drops the call; flush runs it now, once", () => {
    const fn = vi.fn();
    const d = createDebouncer(fn, 500);
    d.schedule();
    d.cancel();
    vi.advanceTimersByTime(1000);
    expect(fn).not.toHaveBeenCalled();

    d.flush();
    expect(fn).not.toHaveBeenCalled();
    d.schedule();
    d.flush();
    expect(fn).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(1000);
    expect(fn).toHaveBeenCalledTimes(1);
  });
});
