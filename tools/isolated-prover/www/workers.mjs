// Stock wasm_thread has no pool-shutdown API. Track only workers created by
// this realm so retiring an entire instance also retires its rayon pool.
const NativeWorker = globalThis.Worker;
const owned = new Set();
let stopping = false;
const failed = reason => {
  if (!stopping)
    dispatchEvent(new CustomEvent("prover-worker-failed", { detail: reason }));
};
globalThis.Worker = class extends NativeWorker {
  constructor(...args) {
    super(...args);
    owned.add(this);
    this.addEventListener("error", event => {
      event.preventDefault(); // Forward the failure once, to the owning request client.
      failed(`worker error: ${event.message}`);
    });
    this.addEventListener("messageerror", () => failed("worker message error"));
  }
  terminate() {
    owned.delete(this);
    super.terminate();
    // wasm_thread's module support probe creates and terminates an empty
    // Worker. Only termination after work was sent kills a running request.
    if (this.hasWork) failed("worker terminated");
  }
  postMessage(...args) {
    this.hasWork = true;
    super.postMessage(...args);
  }
};
export function stopWorkers() {
  stopping = true;
  const count = owned.size;
  for (const worker of owned) worker.terminate();
  return count;
}
window.stopProverWorkers = stopWorkers;
