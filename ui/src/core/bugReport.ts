export interface ReportSections {
  description: string;
  environment: string;
  context: string;
  events: string[];
}

export const BUG_REPORT_INSTRUCTIONS = `This report is too long to fill in automatically.
In Gall, choose **Save report...** to save a ZIP containing the report and its screenshots or videos. Drag the ZIP here, then replace these instructions with a short summary.
Extra diagnostics are optional and available only through Save report in Gall. Review before submitting: issues and uploaded attachments are public.`;

export function reportBodyPreview(report: { body: string; description: string }, description: string) {
  return `## What happened?\n${description}${report.body.slice("## What happened?\n".length + report.description.length)}`;
}

export function reproduction(description: string, steps: string, expected: string): string {
  return [
    description.trim(),
    steps.trim() && `### Steps to reproduce\n${steps.trim()}`,
    expected.trim() && `### Expected behavior\n${expected.trim()}`,
  ].filter(Boolean).join("\n\n");
}

export type AnnotationTool = "arrow" | "rectangle" | "redact";
export interface Mark {
  tool: AnnotationTool;
  x: number;
  y: number;
  endX: number;
  endY: number;
}

export function normalizedBox(mark: Mark) {
  return { x: Math.min(mark.x, mark.endX), y: Math.min(mark.y, mark.endY), width: Math.abs(mark.endX - mark.x), height: Math.abs(mark.endY - mark.y) };
}

export function moveMark(mark: Mark, key: string, resize = false): Mark {
  const dx = key === "ArrowLeft" || key === "h" ? -0.01 : key === "ArrowRight" || key === "l" ? 0.01 : 0;
  const dy = key === "ArrowUp" || key === "k" ? -0.01 : key === "ArrowDown" || key === "j" ? 0.01 : 0;
  const clamp = (n: number) => Math.max(0, Math.min(1, n));
  if (resize) return { ...mark, endX: clamp(mark.endX + dx), endY: clamp(mark.endY + dy) };
  const x = Math.max(-Math.min(mark.x, mark.endX), Math.min(1 - Math.max(mark.x, mark.endX), dx));
  const y = Math.max(-Math.min(mark.y, mark.endY), Math.min(1 - Math.max(mark.y, mark.endY), dy));
  return { ...mark, x: mark.x + x, y: mark.y + y, endX: mark.endX + x, endY: mark.endY + y };
}

export function annotationKey(key: string, vim: boolean): boolean {
  return key.startsWith("Arrow") || (vim && ["h", "j", "k", "l"].includes(key));
}

export function issueLink(title: string, body: string) {
  const heading = Array.from(title).slice(0, 200).join("");
  const urlFor = (text: string) => `https://github.com/GaMa87/gall/issues/new?${new URLSearchParams({ template: "bug_report_app.md", title: heading, body: text })}`;
  const url = urlFor(body);
  const remaining = 7000 - url.length;
  const needsAttachment = remaining < 0;
  return {
    url: needsAttachment ? urlFor(BUG_REPORT_INSTRUCTIONS) : url,
    body: needsAttachment ? BUG_REPORT_INSTRUCTIONS : body,
    needsAttachment,
    remaining,
  };
}
