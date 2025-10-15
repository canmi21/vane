/* pkg/totp/totp.d.ts */

export function wasm_generate_combined_token(
	seeds: string[],
	time: bigint,
	window: bigint
): string;
export function wasm_verify_combined_token(
	seeds: string[],
	time: bigint,
	token: string,
	window: bigint,
	allowed_windows: number,
	unit: string
): boolean;

export type InitInput =
	| RequestInfo
	| URL
	| Response
	| BufferSource
	| WebAssembly.Module;

export interface InitOutput {
	readonly memory: WebAssembly.Memory;
	readonly wasm_generate_combined_token: (
		a: number,
		b: number,
		c: bigint,
		d: bigint
	) => [number, number];
	readonly wasm_verify_combined_token: (
		a: number,
		b: number,
		c: bigint,
		d: number,
		e: number,
		f: bigint,
		g: number,
		h: number,
		i: number
	) => number;
	readonly __wbindgen_malloc: (a: number, b: number) => number;
	readonly __wbindgen_realloc: (
		a: number,
		b: number,
		c: number,
		d: number
	) => number;
	readonly __wbindgen_export_2: WebAssembly.Table;
	readonly __externref_table_alloc: () => number;
	readonly __wbindgen_free: (a: number, b: number, c: number) => void;
	readonly __wbindgen_start: () => void;
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
export function initSync(
	module: { module: SyncInitInput } | SyncInitInput
): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init(
	module_or_path?:
		| { module_or_path: InitInput | Promise<InitInput> }
		| InitInput
		| Promise<InitInput>
): Promise<InitOutput>;
