import init, { MigrationRunner, Schema } from "../../pkg/gaman_wasm.js";
import "./style.css";

const migrationsKey = "gaman-demo-migrations-v2";
const trackingKey = "gaman-demo-tracking";
const output = document.querySelector<HTMLDivElement>("#output")!;
const editor = document.querySelector<HTMLTextAreaElement>("#schema")!;
const prompt = document.querySelector<HTMLFormElement>("#prompt")!;
const command = document.querySelector<HTMLInputElement>("#command")!;

const read = <T>(key: string, fallback: T): T => {
  const value = localStorage.getItem(key);
  return value ? JSON.parse(value) : fallback;
};

const callbacks = {
  migrations: {
    load: () => read(migrationsKey, []),
    save: (migration: unknown) => {
      const migrations = read<unknown[]>(migrationsKey, []);
      migrations.push(migration);
      localStorage.setItem(migrationsKey, JSON.stringify(migrations));
    },
  },
  tracking: {
    install: () => undefined,
    appliedIds: () => read<string[]>(trackingKey, []),
    record: (id: string) => {
      const ids = new Set(read<string[]>(trackingKey, []));
      ids.add(id);
      localStorage.setItem(trackingKey, JSON.stringify([...ids]));
    },
    unrecord: (id: string) => {
      localStorage.setItem(trackingKey, read<string[]>(trackingKey, []).filter((known) => known !== id));
    },
  },
  executor: {
    begin: () => append("BEGIN"),
    execute: (sql: string) => append(sql),
    commit: () => append("COMMIT"),
    rollback: () => append("ROLLBACK"),
  },
};

function append(line: string) {
  output.textContent += `\n${line}`;
}

type CommandResponse = {
  protocol_version: number;
  result: {
    result: string;
    value: unknown;
  };
};

function formatValue(value: unknown): string {
  if (typeof value === "string") return value;
  if (Array.isArray(value)) return value.map(formatValue).join("\n");
  return JSON.stringify(value, null, 2);
}

function renderResult(response: CommandResponse): string {
  const value = response.value;
  if (response.result === "make" && value && typeof value === "object") {
    const result = value as { outcome?: string; migration?: { id?: string } };
    if (result.outcome === "created") return `Created: ${result.migration?.id ?? "migration"}`;
    if (result.outcome === "preview") return `Preview: ${result.migration?.id ?? "migration"}`;
    if (result.outcome === "no_changes") return "No changes detected.";
    if (result.outcome === "check_passed") return "Schema is up to date.";
  }
  if (response.result === "status" && Array.isArray(value)) {
    return value.map(({ id, applied }) => `  [${applied ? "X" : " "}] ${id}`).join("\n") || "No migrations found.";
  }
  if (response.result === "pending" && Array.isArray(value)) {
    return value.length ? value.map((id) => `  ${id}`).join("\n") : "No pending migrations.";
  }
  if (response.result === "movement" && value && typeof value === "object") {
    const movement = value as { applied?: number; reverted?: number };
    const applied = movement.applied ?? 0;
    const reverted = movement.reverted ?? 0;
    if (!applied && !reverted) return "No migration state changes.";
    return [
      applied ? `Applied ${applied} migration${applied === 1 ? "" : "s"}.` : "",
      reverted ? `Reverted ${reverted} migration${reverted === 1 ? "" : "s"}.` : "",
    ].filter(Boolean).join("\n");
  }
  if (response.result === "show" && Array.isArray(value)) {
    return value.map(({ id, content }) => `--- ${id}\n${content}`).join("\n");
  }
  if (response.result === "sql" && Array.isArray(value)) return value.join("\n");
  return formatValue(value);
}

function renderError(error: unknown): string {
  if (error && typeof error === "object") {
    const value = error as { lines?: string[]; message?: string };
    if (value.lines) return value.lines.join("\n");
    if (value.message) return value.message;
  }
  return String(error);
}

function configOutput() {
  return [
    "dialect: postgres",
    "schema: browser editor",
    "migrations: localStorage",
    "tracking: localStorage",
    "executor: browser callback",
  ].join("\n");
}

function boot() {
  const migrator = init().then(() => new MigrationRunner("postgres", callbacks));

  prompt.addEventListener("submit", async (event) => {
    event.preventDefault();
    try {
      const tokens = command.value.trim().split(/\s+/).filter(Boolean);
      if (tokens[0] === "help") {
        output.textContent = MigrationRunner.commandHelp(tokens[1] ?? null);
        return;
      }
      if (tokens[0] === "config") {
        output.textContent = configOutput();
        return;
      }
      const runner = await migrator;
      runner.set_schema(Schema.fromSql(editor.value, "postgres"));
      const result = await runner.runTokens(tokens, undefined) as CommandResponse;
      output.textContent = renderResult(result.result);
    } catch (error) {
      output.textContent = renderError(error);
    }
  });
}

boot();
