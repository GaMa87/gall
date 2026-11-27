import { Fragment, useEffect, useState } from "react";
import { api } from "../api/ipc";
import type { SyntaxHighlight } from "../api/types";
import { segments } from "../core/diffRows";

/** Colour complete source files so multiline syntax retains its context. Ignore stale replies after navigation. */
export function useMergeHighlight(name: string, lines: string[]): SyntaxHighlight | null {
  const [found, setFound] = useState<{ name: string; lines: string[]; value: SyntaxHighlight | null } | null>(null);
  useEffect(() => {
    let alive = true;
    const timer = setTimeout(() => {
      const input = lines.map((line) => line.replace(/\r?\n$/, ""));
      void api.diffHighlight(name, input).then((value) => {
        if (alive) setFound({ name, lines, value: value ?? gitignoreHighlight(name, input) });
      }).catch(() => { if (alive) setFound({ name, lines, value: gitignoreHighlight(name, input) }); });
    }, 120);
    return () => { alive = false; clearTimeout(timer); };
  }, [name, lines]);
  return found?.name === name && found.lines === lines ? found.value : null;
}

/** The syntax service may not have a grammar for this common configuration file. */
export function gitignoreHighlight(name: string, lines: string[]): SyntaxHighlight | null {
  if (!/(^|[\\/])\.gitignore$/i.test(name)) return null;
  return { language: "gitignore", lines: lines.map((line) => {
    if (line.startsWith("#")) return [[0, line.length, 1]];
    const runs: [number, number, number][] = [];
    for (let i = 0; i < line.length; i++) {
      if (line[i] === "\\") { i++; continue; }
      if ((i === 0 && line[i] === "!") || "*?[]".includes(line[i])) runs.push([i, i + 1, 5]);
    }
    return runs;
  }) };
}

/** Render text as React nodes, retaining every line terminator and never interpreting file text as HTML. */
export function colouredLine(line: string, syntax: SyntaxHighlight | null | undefined, index: number) {
  const body = line.replace(/\r?\n$/, ""), ending = line.slice(body.length);
  return <>{segments(body, undefined, syntax?.lines[index]).map(([text, , category], i) => category ? <span key={i} className={`sx${category}`}>{text}</span> : <Fragment key={i}>{text}</Fragment>)}{ending}</>;
}
