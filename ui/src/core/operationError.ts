// Classify command failures consistently in notifications and bug-report diagnostics.
import { basename, parent } from "./path";

type OperationError = {
  text: string;
  kind: "info" | "warn" | "error";
  reportable: boolean;
};

const IO = /^([a-z][a-z -]*) failed for '(.+)': (.+)$/is;
const EXISTS = /^'(.+)' (already exists|does not exist)$/s;
const PASSWORD = /^archive-password:(needed|wrong):(.+)$/s;

// Rust includes the OS code even when the accompanying message is localized.
const OS_ERRORS = [
  { windows: [5, 1314], unix: [1, 13], text: /Permission denied|Access is denied|Operation not permitted|required privilege is not held/i, reason: "you have no permission" },
  { windows: [32, 33], unix: [16, 26], text: /being used by another process|locked a portion|resource busy|Text file busy/i, reason: "it is in use" },
  { windows: [2, 3], unix: [2], text: /No such file or directory|cannot find the (path|file)/i, reason: "it no longer exists" },
  { windows: [80, 183], unix: [17], text: /File exists|already exists/i, reason: "it already exists" },
  { windows: [39, 112], unix: [28, 122], macos: [28, 69], text: /not enough space|No space left|quota exceeded/i, reason: "the drive is full" },
  { windows: [206], unix: [36], macos: [63], text: /name too long|path is too long/i, reason: "the name or path is too long" },
  { windows: [19], unix: [30], text: /Read-only file system|write.?protected/i, reason: "the drive is read-only" },
  { windows: [267], unix: [20], text: /Not a directory|the directory name is invalid/i, reason: "it is not a folder" },
] as const;

function osReason(detail: string, platform?: string): string | undefined {
  const match = /\(os error (\d+)\)\s*$/.exec(detail);
  if (match && platform) {
    const code = Number(match[1]);
    const reason = OS_ERRORS.find((e) => {
      const codes = platform === "windows" ? e.windows : platform === "macos" && "macos" in e ? e.macos : e.unix;
      return codes.some((n) => n === code);
    });
    if (reason) return reason.reason;
  }
  return OS_ERRORS.find((e) => e.text.test(detail))?.reason;
}

export function isMissingError(raw: string, platform?: string): boolean {
  const io = IO.exec(raw);
  return EXISTS.exec(raw)?.[2] === "does not exist" || osReason(io?.[3] ?? raw, platform) === "it no longer exists";
}

export function operationError(raw: string, platform?: string): OperationError {
  const expected = (text: string, kind: OperationError["kind"] = "error"): OperationError => ({ text, kind, reportable: false });
  const unexpected = (text: string): OperationError => ({ text, kind: "error", reportable: true });

  if (raw === "operation cancelled") return expected("Operation cancelled", "info");
  if (raw === "nothing to paste") return expected("Nothing to paste", "info");
  if (raw === "nothing to drag") return expected("Nothing to drag", "info");
  if (raw.startsWith("no-trash: ")) return expected(raw.slice("no-trash: ".length));
  const password = PASSWORD.exec(raw);
  if (password) {
    const name = basename(password[2]);
    return expected(password[1] === "wrong" ? `Wrong password for “${name}”. Open it to try again.`
      : `“${name}” is encrypted. Open it and enter the password, then try again.`, "warn");
  }

  const exists = EXISTS.exec(raw);
  if (exists) {
    const dir = parent(exists[1]);
    return expected(`“${basename(exists[1]) || exists[1]}” ${exists[2]}${dir ? ` in “${basename(dir) || dir}”` : ""}`, "warn");
  }
  const invalidName = /^invalid name '(.*)'$/s.exec(raw);
  if (invalidName) {
    const rules = platform === "windows" ? 'Names cannot contain < > : " / \\ | ? * or end with a dot or space.' : "Use a single name without a slash.";
    return expected(`“${invalidName[1]}” is not a valid name. ${rules}`, "warn");
  }
  const intoItself = /^cannot (\w+): '(.+)' is inside '(.+)'$/s.exec(raw);
  if (intoItself) return expected(`Cannot ${intoItself[1]} “${basename(intoItself[3])}” into itself.`, "warn");

  const io = IO.exec(raw);
  if (io) {
    const [, op, path, detail] = io;
    const name = `“${basename(path) || path}”`;
    const reason = osReason(detail, platform);
    if (op === "list" && reason === "you have no permission") return expected(`You have no permission to open ${name}.`);
    if (op === "list" && reason === "it no longer exists") return expected(`${name} no longer exists.`);
    if (op === "list" && reason === "it is not a folder") return expected(`${name} is not a folder.`);
    const action = op === "list" ? "open" : op === "stat" ? "access" : op;
    const message = `Cannot ${action} ${name}`;
    if (reason) return expected(`${message}: ${reason}.`);
    return unexpected(`${message}: ${detail.replace(/ \(os error \d+\)$/, "")}`);
  }

  if (/^two items would be named |^“.+” cannot be replaced: it contains |^Nothing was \w+: cannot \w+ \d+\+? items? inside WSL with |^“.+” cannot be changed from Windows: a name in its path ends with /is.test(raw)) {
    return expected(raw[0].toUpperCase() + raw.slice(1), "warn");
  }
  if (raw.startsWith("cannot safely restore conflicting entry ")) return expected("C" + raw.slice(1));

  if (/^(?:Wrong passphrase|Not authorized to mount this volume|not an encrypted volume|unlocking is not supported here)$| is not mounted$/s.test(raw)) {
    return expected(raw[0].toUpperCase() + raw.slice(1), "warn");
  }

  // These errors describe applications, archives, devices or system services.
  if (/^(?:no terminal found;|application .+ not found$|choose the application in the system dialog$)/i.test(raw)) {
    return expected(raw[0].toUpperCase() + raw.slice(1), "warn");
  }
  if (/^(?:trash:|zip:|7z:|tar:|zstd:|clipboard unavailable:|cannot start '|windows shortcut chain is cyclic or too long$|git does not refuse this folder$|error: could not (?:lock|write) config file |eject failed: the drive is in use by )/i.test(raw)
    || / is not (?:a supported archive|an archive Gall can browse)$/.test(raw)
    || osReason(raw, platform)) {
    return expected(raw);
  }
  return unexpected(raw);
}

/** Filesystem mount errors from udisks are expected machine-setup or volume-state issues. */
export function volumeMountError(raw: string, fs: string): (OperationError & { details: string }) | undefined {
  if (/no such file or directory/i.test(raw)) {
    return {
      text: "Could not find volume “{name}”. Check that the drive is connected and try again.",
      kind: "warn", reportable: false, details: raw,
    };
  }
  if (!/wrong fs type|bad superblock|missing codepage|helper program|unknown filesystem/i.test(raw)) {
    return undefined;
  }
  const filesystem = fs.toLowerCase();
  let text: string;
  if (filesystem === "ntfs") {
    text = `Could not mount “{name}”. The NTFS volume may have been left hibernated by Windows or may need checking. Fully shut down Windows and disable Fast Startup; if it still will not mount, run chkdsk in Windows. If this system has no NTFS support, install ntfs-3g and ntfsprogs.`;
  } else if (filesystem === "exfat") {
    text = `Could not mount “{name}”. Check that exFAT support (exfatprogs) is installed and that the volume is healthy.`;
  } else {
    text = `Could not mount “{name}”. The {fs} filesystem may need its system driver or repair. Check that filesystem support is installed and that the volume is healthy.`;
  }
  return { text, kind: "warn", reportable: false, details: raw };
}
