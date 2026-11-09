import type { ReportMedia } from "./reportMedia";

export interface ReportDraft {
  title: string; description: string; steps: string; expected: string; includeLog: boolean;
  media: ReportMedia[]; capturing: boolean; captureError: string;
}
const emptyDraft = (): ReportDraft => ({ title: "", description: "", steps: "", expected: "", includeLog: true, media: [], capturing: false, captureError: "" });
let draft = emptyDraft();
let handedOff = false;
const listeners = new Set<() => void>();
export const getReportDraft = () => draft;
export function subscribeReportDraft(listener: () => void) {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}
export function updateReportDraft(patch: Partial<ReportDraft> | ((current: ReportDraft) => Partial<ReportDraft>)) {
  draft = { ...draft, ...(typeof patch === "function" ? patch(draft) : patch) };
  listeners.forEach((listener) => listener());
}
export function completeReportDraft() {
  handedOff = true;
}
export function beginReportDraft(resume = false) {
  if (resume || !handedOff || draft.capturing) return;
  handedOff = false;
  draft = emptyDraft();
  listeners.forEach((listener) => listener());
}
