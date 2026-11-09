/** What `y` can copy about a commit, like lazygit's "Copy commit attribute to clipboard" menu. */
export type CommitCopyInfo = { id: string; author: string; email: string; date: string; message: string; tags: string[]; url: string | null };
export type CopyChoice = { label: string; text: string | (() => Promise<string>) };

/** Subject is the first line; the body is everything after the blank line that follows it. */
export function splitMessage(message: string): { subject: string; body: string } {
  const text = message.replace(/\r\n/g, "\n").replace(/\s+$/, "");
  const at = text.indexOf("\n");
  if (at < 0) return { subject: text, body: "" };
  return { subject: text.slice(0, at), body: text.slice(at + 1).replace(/^\n+/, "") };
}

/** Web URL of a commit for the well-known hosts; null for anything else (self-hosted, local paths). */
export function commitUrl(remoteUrl: string, id: string): string | null {
  const url = remoteUrl.trim();
  const match = /^(?:https?:\/\/(?:[^@/]+@)?|ssh:\/\/(?:[^@/]+@)?|git:\/\/|git@)([^/:]+)(?::\d+)?[/:](.+?)(?:\.git)?\/?$/.exec(url);
  if (!match) return null;
  const host = match[1].toLowerCase(), repository = match[2].replace(/^\/+/, "");
  if (!/^[\w.-]+\/[\w./-]+$/.test(repository)) return null;
  if (host === "github.com" || host === "codeberg.org") return `https://${host}/${repository}/commit/${id}`;
  if (host === "gitlab.com") return `https://${host}/${repository}/-/commit/${id}`;
  if (host === "bitbucket.org") return `https://${host}/${repository}/commits/${id}`;
  return null;
}

/** The menu entries, in lazygit's order; entries that would copy nothing are left out. */
export function copyChoices(info: CommitCopyInfo, diff: () => Promise<string>): CopyChoice[] {
  const { subject, body } = splitMessage(info.message);
  const choices: CopyChoice[] = [
    { label: "Commit hash", text: info.id },
    { label: "Abbreviated hash", text: info.id.slice(0, 8) },
    { label: "Commit subject", text: subject },
  ];
  if (body) choices.push({ label: "Commit message body", text: body });
  choices.push({ label: "Full commit message", text: info.message.replace(/\s+$/, "") });
  choices.push({ label: "Author", text: info.email ? `${info.author} <${info.email}>` : info.author });
  choices.push({ label: "Date", text: info.date });
  if (info.url) choices.push({ label: "Commit URL", text: info.url });
  choices.push({ label: "Commit diff", text: diff });
  if (info.tags.length) choices.push({ label: "Tags", text: info.tags.join("\n") });
  return choices;
}
