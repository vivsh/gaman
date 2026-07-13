/* tslint:disable */
/* eslint-disable */

/**
 * Browser migration facade. `callbacks` may provide nested `migrations`,
 * `tracking`, and `executor` objects whose methods return values or Promises.
 */
export class Migrator {
    free(): void;
    [Symbol.dispose](): void;
    constructor(dialect: string, callbacks: any);
    /**
     * Runs the browser shell command and returns structured terminal output.
     */
    run(command_line: string, decisions: any): Promise<any>;
    set_schema(schema: Schema): void;
}

export class Schema {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    static fromJson(source: string, dialect: string): Schema;
    static fromSql(source: string, dialect: string): Schema;
    static fromYaml(source: string, dialect: string): Schema;
}

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_migrator_free: (a: number, b: number) => void;
    readonly __wbg_schema_free: (a: number, b: number) => void;
    readonly migrator_new: (a: number, b: number, c: number, d: number) => void;
    readonly migrator_run: (a: number, b: number, c: number, d: number) => number;
    readonly migrator_set_schema: (a: number, b: number) => void;
    readonly schema_fromJson: (a: number, b: number, c: number, d: number, e: number) => void;
    readonly schema_fromSql: (a: number, b: number, c: number, d: number, e: number) => void;
    readonly schema_fromYaml: (a: number, b: number, c: number, d: number, e: number) => void;
    readonly __wasm_bindgen_func_elem_351: (a: number, b: number, c: number, d: number) => void;
    readonly __wasm_bindgen_func_elem_362: (a: number, b: number, c: number, d: number) => void;
    readonly __wbindgen_export: (a: number, b: number) => number;
    readonly __wbindgen_export2: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_export3: (a: number) => void;
    readonly __wbindgen_export4: (a: number, b: number) => void;
    readonly __wbindgen_add_to_stack_pointer: (a: number) => number;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
