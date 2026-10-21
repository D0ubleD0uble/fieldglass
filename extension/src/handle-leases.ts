// Closing a native handle as soon as nothing holds it (#1204).
//
// A handle owns its file's bytes, its decoded-value memos and, since #1233, a
// warp plan of up to 256 MiB, all in native memory that V8 does not count. So
// leaving a dropped handle to the garbage collector can keep that memory for as
// long as the collector sees no reason to run. Each place that keeps a handle
// holds a lease on it instead, and the last lease let go calls `close()`.
//
// The holders are the provider's handle maps (one lease per entry), each render
// panel's `PanelHandleGate` (the handle its page was written from, until it
// adopts another), and an export that holds the handle across a dialog.

/** Something that can be closed: the addon's `Handle`. */
export interface Closable {
  close(): void;
}

/** Who holds each handle. Holding twice with the same holder counts once, and
 *  releasing what is not held does nothing, so a holder cannot close a handle
 *  another still holds. */
export class HandleLeases<H extends Closable> {
  private readonly holders = new Map<H, Set<object>>();

  hold(handle: H, holder: object): void {
    let held = this.holders.get(handle);
    if (!held) this.holders.set(handle, (held = new Set()));
    held.add(holder);
  }

  /** Let go of `handle`, closing it if `holder` was the last to hold it. */
  release(handle: H | undefined, holder: object): void {
    if (!handle) return;
    const held = this.holders.get(handle);
    if (!held?.delete(holder) || held.size > 0) return;
    this.holders.delete(handle);
    try {
      handle.close();
    } catch (err) {
      // `close()` does not throw today. Should it, the handle is dropped all
      // the same, and nothing that held it is left to report to.
      console.error("[Fieldglass] Handle.close failed:", err);
    }
  }

  /** How many holders `handle` has, for tests. */
  holderCount(handle: H): number {
    return this.holders.get(handle)?.size ?? 0;
  }
}

/** A map of handles by key that holds a lease on each value while it is in the
 *  map: replacing or deleting an entry lets go of the handle it held. A handle
 *  is kept under one key at a time. */
export class HeldHandles<H extends Closable> extends Map<string, H> {
  constructor(private readonly leases: HandleLeases<H>) {
    super();
  }

  override set(key: string, handle: H): this {
    const old = super.get(key);
    super.set(key, handle);
    // Held before the old one is let go, in case they are the same.
    this.leases.hold(handle, this);
    if (old !== handle) this.leases.release(old, this);
    return this;
  }

  override delete(key: string): boolean {
    const old = super.get(key);
    const had = super.delete(key);
    this.leases.release(old, this);
    return had;
  }

  override clear(): void {
    const old = [...this.values()];
    super.clear();
    for (const handle of old) this.leases.release(handle, this);
  }
}
