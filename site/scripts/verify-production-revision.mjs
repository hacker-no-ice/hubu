import { pathToFileURL } from "node:url";

const DEFAULT_ATTEMPTS = 18;
const DEFAULT_DELAY_MS = 5_000;

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

export async function waitForRevision({
  endpoint,
  expectedRevision,
  attempts = DEFAULT_ATTEMPTS,
  delayMs = DEFAULT_DELAY_MS,
  fetchImpl = globalThis.fetch,
  sleep = delay,
}) {
  if (!endpoint || !expectedRevision) {
    throw new Error("A production endpoint and expected revision are required.");
  }

  let lastResult = "no response";

  for (let attempt = 1; attempt <= attempts; attempt += 1) {
    try {
      const response = await fetchImpl(endpoint, {
        cache: "no-store",
        headers: { "cache-control": "no-cache" },
      });
      const body = (await response.text()).trim();
      lastResult = response.ok
        ? body.replaceAll(/\s+/g, " ").slice(0, 200) || "empty response"
        : `HTTP ${response.status}`;

      if (response.ok && body === expectedRevision) {
        return;
      }
    } catch (error) {
      lastResult = error instanceof Error ? error.message : String(error);
    }

    if (attempt < attempts) {
      await sleep(delayMs);
    }
  }

  throw new Error(
    `Expected production revision ${expectedRevision} after ${attempts} attempts; last result: ${lastResult}`,
  );
}

async function main() {
  const [endpoint, expectedRevision] = process.argv.slice(2);
  await waitForRevision({ endpoint, expectedRevision });
  console.log(`Verified production revision ${expectedRevision}.`);
}

const invokedPath = process.argv[1] ? pathToFileURL(process.argv[1]).href : "";
if (import.meta.url === invokedPath) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : error);
    process.exitCode = 1;
  });
}
