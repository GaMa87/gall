// Lines of a text as the merge core cuts them: each keeps its terminator (`\n` or `\r\n`), and a last line without one stays
// as it is, so joining the lines gives back the text exactly. The merge itself is in `gall-core::merge` (IPC `mergeThreeWay`),
// its cards and result in `mergeCards.ts`.
export const splitLines = (text: string): string[] => text.match(/[^\n]*\n|[^\n]+$/g) ?? [];
