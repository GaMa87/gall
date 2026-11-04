// Display formatting helpers.

const UNITS = ["B", "KB", "MB", "GB", "TB", "PB"];

export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  let v = bytes;
  let u = 0;
  while (v >= 1024 && u < UNITS.length - 1) {
    v /= 1024;
    u++;
  }
  return `${v >= 100 ? v.toFixed(0) : v >= 10 ? v.toFixed(1) : v.toFixed(2)} ${UNITS[u]}`;
}

const pad = (n: number) => String(n).padStart(2, "0");

export function formatDate(ms: number | null): string {
  if (ms == null) return "";
  const d = new Date(ms);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

export function formatTime(ms: number | null): string {
  if (ms == null) return "";
  const d = new Date(ms);
  return `${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

export function formatDateTime(ms: number | null): string {
  if (ms == null) return "";
  const d = new Date(ms);
  return `${formatDate(ms)} ${formatTime(ms)}:${pad(d.getSeconds())}`;
}

/** `ls -l` permissions of a Unix `st_mode`: "drwxr-xr-x", "lrwxrwxrwx", "drwxrwxrwt". */
export function unixMode(mode: number): string {
  const type = ({ 0o040000: "d", 0o120000: "l", 0o020000: "c", 0o060000: "b", 0o010000: "p", 0o140000: "s" } as Record<number, string>)[mode & 0o170000] ?? "-";
  const trio = (shift: number, special: boolean, on: string) => {
    const r = mode & (0o4 << shift) ? "r" : "-";
    const w = mode & (0o2 << shift) ? "w" : "-";
    const x = !!(mode & (0o1 << shift));
    return r + w + (special ? (x ? on : on.toUpperCase()) : x ? "x" : "-");
  };
  return type + trio(6, !!(mode & 0o4000), "s") + trio(3, !!(mode & 0o2000), "s") + trio(0, !!(mode & 0o1000), "t");
}

/** PowerShell 7's `Mode` of Windows file attributes: "d----", "-a---", "-ar--", "la---"
 *  (link / folder, archive, read-only, hidden, system). */
export function windowsMode(attrs: number, isDir: boolean): string {
  const bit = (b: number, c: string) => (attrs & b ? c : "-");
  return (attrs & 0x400 ? "l" : isDir ? "d" : "-") + bit(0x20, "a") + bit(0x1, "r") + bit(0x2, "h") + bit(0x4, "s");
}
