// The offset a user types into the hex comparison: decimal, or hex with 0x, or "end". Pure and strict, so a typo or an
// unsafe number never scrolls somewhere unexpected and never reaches the backend as a read position.

/** `reason` is a text for `tt`; the one about the end has a `{size}` to fill in. */
export type OffsetResult = { ok: true; offset: number } | { ok: false; reason: string };

const DECIMAL = /^[0-9]+$/;
const HEX = /^0[xX][0-9a-fA-F]+$/;

/**
 * Reads an offset into a file of `size` bytes. Accepted: `0`, `4096`, `0x1000`, `0X1f`, and `end` / `eof` (the end of
 * the larger file). Not accepted: signs, fractions, digit separators, hex digits without `0x` (`1F` is not 31), a
 * number beyond the end (`size` itself is the end, and the only offset of an empty file), and a number that is not a
 * safe integer. The offset of the end is `size`: show the last row, not a row past it (`rowOfOffset`).
 */
export function parseOffset(text: string, size: number): OffsetResult {
  const raw = text.trim();
  if (!raw) return { ok: false, reason: "Enter an offset: a number, 0x followed by hex digits, or end." };
  if (/^(end|eof)$/i.test(raw)) return { ok: true, offset: size };
  let value: bigint;
  if (HEX.test(raw)) value = BigInt(raw);
  else if (DECIMAL.test(raw)) value = BigInt(raw);
  else return { ok: false, reason: "Not an offset: use digits, or 0x followed by hex digits, or end." };
  if (value > BigInt(Number.MAX_SAFE_INTEGER)) return { ok: false, reason: "That number is too large to be an offset." };
  const offset = Number(value);
  if (offset > size) return { ok: false, reason: "Beyond the end of the file ({size} bytes)." };
  return { ok: true, offset };
}

/** The row to bring into view for an offset: the end of a file is its last row, an empty file has row 0. */
export function rowOfOffset(offset: number, size: number, perRow: number): number {
  return Math.floor(Math.min(Math.max(offset, 0), Math.max(size - 1, 0)) / perRow);
}
