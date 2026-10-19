import { spawnSync } from "node:child_process";
import { prepareAnalyticsDeployment } from "./configure-analytics.mjs";

const { config, configPath } = await prepareAnalyticsDeployment();
if (config.vars.ANALYTICS_ENABLED !== "true") {
  console.log("Analytics is disabled; no remote migrations applied.");
} else {
  const result = spawnSync(process.execPath, ["node_modules/wrangler/bin/wrangler.js", "d1", "migrations", "apply", "ANALYTICS_DB", "--remote", "--config", configPath], {
    stdio: "inherit",
    env: { ...process.env, XDG_CONFIG_HOME: ".wrangler/config", WRANGLER_LOG_PATH: ".wrangler/wrangler.log" },
  });
  if (result.error) throw result.error;
  process.exitCode = result.status ?? 1;
}
