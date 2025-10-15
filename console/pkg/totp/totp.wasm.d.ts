/* pkg/totp/totp.wasm.d.ts */

export const memory: WebAssembly.Memory;
export const wasm_generate_combined_token: (
	a: number,
	b: number,
	c: bigint,
	d: bigint
) => [number, number];
export const wasm_verify_combined_token: (
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
export const __wbindgen_malloc: (a: number, b: number) => number;
export const __wbindgen_realloc: (
	a: number,
	b: number,
	c: number,
	d: number
) => number;
export const __wbindgen_export_2: WebAssembly.Table;
export const __externref_table_alloc: () => number;
export const __wbindgen_free: (a: number, b: number, c: number) => void;
export const __wbindgen_start: () => void;
