// Ids of requests that can be cancelled from the window (the local model's explanation and proposals): increasing in
// this window and, through the time and a random part, different from those of another window.
let last = 0;
export function nextRequestId(): number {
  last = Math.max(last + 1, Date.now() * 1000 + Math.floor(Math.random() * 1000));
  return last;
}
