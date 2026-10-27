// A request belongs to one iframe generation. Retirement rejects every waiter.
export const MAX_BYTES = 1024 * 1024;
const clock = () => performance.timeOrigin + performance.now();
export class ProverHost {
  constructor(timeoutMs = 900000) {
    this.timeoutMs = timeoutMs;
    this.pending = new Map();
    this.nextId = 0;
    this.frame = document.createElement("iframe");
    this.frame.hidden = true;
    this.frame.title = "Isolated prover host";
    this.ready = new Promise((resolve, reject) => {
      this.readyResolve = resolve;
      this.readyReject = reject;
    });
    this.readyTimer = setTimeout(() => this.fail("host load timeout"), 30000);
    this.onMessage = event => {
      if (
        event.origin !== location.origin ||
        event.source !== this.frame.contentWindow
      )
        return;
      const data = event.data;
      if (data.op === "host-ready") {
        clearTimeout(this.readyTimer);
        this.readyResolve();
        return;
      }
      if (data.op === "host-failed") return this.fail(data.error);
      const item = this.pending.get(data.id);
      if (!item) return;
      clearTimeout(item.timer);
      this.pending.delete(data.id);
      if (
        data.proof &&
        (!(data.proof instanceof ArrayBuffer) ||
          data.proof.byteLength > MAX_BYTES)
      ) {
        item.reject(new Error("invalid or oversized proof"));
        this.fail("invalid or oversized proof");
        return;
      }
      if (data.sentAt) data.outputTransferMs = clock() - data.sentAt;
      if (data.error) item.reject(new Error(data.error));
      else item.resolve(data);
    };
    addEventListener("message", this.onMessage);
    this.frame.addEventListener("error", () => this.fail("iframe load error"));
    let loads = 0;
    this.frame.addEventListener("load", () => {
      if (++loads > 1) this.fail("iframe navigated");
    });
    this.observer = new MutationObserver(() => {
      if (!this.frame.isConnected) this.fail("iframe removed");
    });
    this.observer.observe(document.body, { childList: true, subtree: true });
    this.frame.src = "prover.html";
    document.body.append(this.frame);
  }
  fail(reason) {
    if (this.dead) return;
    this.dead = true;
    clearTimeout(this.readyTimer);
    this.readyReject(new Error(reason));
    for (const item of this.pending.values()) {
      clearTimeout(item.timer);
      item.reject(new Error(reason));
    }
    this.pending.clear();
    // Stop the remaining pool while its owner is still reachable.
    try {
      this.frame.contentWindow?.stopProverWorkers?.();
    } catch {}
  }
  request(op, fields = {}, transfer = []) {
    if (this.dead)
      return Promise.reject(new Error("prover host retired or failed"));
    if (
      fields.buffer !== undefined &&
      (!(fields.buffer instanceof ArrayBuffer) ||
        fields.buffer.byteLength > MAX_BYTES)
    )
      return Promise.reject(
        new Error("input exceeds 1 MiB limit or is not an ArrayBuffer"),
      );
    return new Promise((resolve, reject) => {
      const id = ++this.nextId;
      const timer = setTimeout(
        () => this.fail(`${op} timed out after ${this.timeoutMs} ms`),
        this.timeoutMs,
      );
      this.pending.set(id, { resolve, reject, timer });
      try {
        this.frame.contentWindow.postMessage(
          { id, op, ...fields },
          location.origin,
          transfer,
        );
      } catch (error) {
        clearTimeout(timer);
        this.pending.delete(id);
        reject(error);
      }
    });
  }
  async retire() {
    const workers = this.frame.contentWindow?.stopProverWorkers?.() ?? 0;
    this.fail("prover host retired");
    // Terminate the pool before discarding the owning realm (I34 teardown fix).
    await new Promise(resolve => setTimeout(resolve, 1000));
    this.observer.disconnect();
    removeEventListener("message", this.onMessage);
    this.frame.remove();
    return workers;
  }
}
