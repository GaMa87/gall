/** Tells a moved mouse from a list that appeared or scrolled under a mouse at rest. The webview sends mouseenter in both cases,
 *  so a menu opened with a key under the pointer would highlight whatever lies there, and Enter would run it. */
export class HoverGuard {
  private last: { x: number; y: number } | null = null;
  /** `first`: what the first event means. True for a list opened with the mouse, false for one opened with a key. */
  constructor(private first: boolean) {}
  reset(first = this.first) { this.last = null; this.first = first; }
  /** Whether the mouse is somewhere else than at the previous event. */
  moved(event: { clientX: number; clientY: number }): boolean {
    const last = this.last;
    this.last = { x: event.clientX, y: event.clientY };
    return last ? last.x !== event.clientX || last.y !== event.clientY : this.first;
  }
}
