import init, { Migrator, Schema } from "../../pkg/gaman_wasm.js";
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

async function boot() {
  await init();
  const migrator = new Migrator("postgres", callbacks);

  prompt.addEventListener("submit", async (event) => {
    event.preventDefault();
    try {
      migrator.set_schema(Schema.fromSql(editor.value, "postgres"));
      const result = await migrator.run(command.value, undefined) as { lines: string[] };
      output.textContent = result.lines.join("\n");
    } catch (error) {
      output.textContent = String(error);
    }
  });
}

void boot();
