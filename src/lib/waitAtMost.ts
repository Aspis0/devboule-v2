/**
 * Resolves when `pending` settles, or after `ms`, whichever comes first. A
 * failure settles it too: the caller then reads the state already stored.
 */
export function waitAtMost(ms: number, pending: Promise<unknown>): Promise<void> {
  return new Promise((resolve) => {
    const timer = setTimeout(resolve, ms);
    const done = () => {
      clearTimeout(timer);
      resolve();
    };
    Promise.resolve(pending).then(done, done);
  });
}
