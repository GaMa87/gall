import { invoke } from "@tauri-apps/api/core";

const CHUNK = 400;
/** Messages by commit; a commit never changes, so they are kept. */
const known = new Map<string, string>();

/** The whole messages of commits, asked for when they are needed. */
export async function commitMessages(path: string, ids: string[]): Promise<Map<string, string>> {
  const missing = [...new Set(ids)].filter((id) => !known.has(id));
  const result = new Map<string, string>();
  for (const id of ids) if (known.has(id)) result.set(id, known.get(id)!);
  for (let i = 0; i < missing.length; i += CHUNK) {
    for (const { id, message } of await invoke<{ id: string; message: string }[]>("git_commit_messages", { path, ids: missing.slice(i, i + CHUNK) })) {
      result.set(id, message);
      known.set(id, message);
    }
  }
  while (known.size > 2000) known.delete(known.keys().next().value!);
  return result;
}
