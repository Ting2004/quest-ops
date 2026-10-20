/** Runs `fn` over `items` with at most `limit` calls in flight. */
export async function runLimited<T>(
  items: T[],
  limit: number,
  fn: (item: T, i: number) => Promise<void>,
) {
  let next = 0;
  const worker = async () => {
    while (next < items.length) {
      const i = next++;
      await fn(items[i], i);
    }
  };
  await Promise.all(Array.from({ length: Math.min(limit, items.length) }, worker));
}

/** Parallel AI requests for batch operations. */
export const AI_CONCURRENCY = 3;
