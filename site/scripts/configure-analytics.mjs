import { readFile, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";

/** Prepare the built bundle only; checked-in configuration remains safe locally. */
export function configureAnalytics(config, settings, migrationsDirectory) {
  const databaseId = settings.HUBUSTACK_ANALYTICS_DB_ID || "";
  const team = settings.HUBUSTACK_ANALYTICS_ACCESS_TEAM_DOMAIN || "";
  const audience = settings.HUBUSTACK_ANALYTICS_ACCESS_AUD || "";
  const configured = Boolean(databaseId || team || audience);
  if (configured && (!/^[a-f0-9]{8}-(?:[a-f0-9]{4}-){3}[a-f0-9]{12}$/i.test(databaseId)
    || !/^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.cloudflareaccess\.com$/.test(team)
    || !audience || audience.trim() !== audience || audience.length > 512)) {
    throw new Error("Set all three analytics deployment variables: HUBUSTACK_ANALYTICS_DB_ID, HUBUSTACK_ANALYTICS_ACCESS_TEAM_DOMAIN, and HUBUSTACK_ANALYTICS_ACCESS_AUD.");
  }
  const prepared = structuredClone(config);
  prepared.vars = { ...prepared.vars, ANALYTICS_ENABLED: configured ? "true" : "false", ANALYTICS_ACCESS_TEAM_DOMAIN: team, ANALYTICS_ACCESS_AUD: audience };
  prepared.d1_databases = (prepared.d1_databases || []).filter((database) => database.binding !== "ANALYTICS_DB");
  if (configured) {
    prepared.d1_databases.push({ binding: "ANALYTICS_DB", database_name: "hubustack-analytics", database_id: databaseId, migrations_dir: migrationsDirectory });
  }
  const crons = (prepared.triggers?.crons || []).filter((cron) => cron !== "17 3 * * *");
  prepared.triggers = { ...prepared.triggers, crons: configured ? [...crons, "17 3 * * *"] : crons };
  return prepared;
}

export async function prepareAnalyticsDeployment(settings = process.env) {
  const configPath = new URL("../dist/server/wrangler.json", import.meta.url);
  const config = JSON.parse(await readFile(configPath, "utf8"));
  const prepared = configureAnalytics(config, settings, fileURLToPath(new URL("../migrations", import.meta.url)));
  await writeFile(configPath, `${JSON.stringify(prepared, null, 2)}\n`);
  return { config: prepared, configPath: fileURLToPath(configPath) };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const { config } = await prepareAnalyticsDeployment();
  console.log(config.vars.ANALYTICS_ENABLED === "true" ? "Analytics deployment configured." : "Analytics disabled: no production analytics configuration supplied.");
}
