import { expect, it } from "vitest";
import { HoverGuard } from "./hover";

const at = (clientX: number, clientY: number) => ({ clientX, clientY });

it("ignores a menu that opens with a key or scrolls under a mouse at rest, and follows the mouse once it moves", () => {
  const guard = new HoverGuard(false);
  expect(guard.moved(at(10, 10))).toBe(false);
  expect(guard.moved(at(10, 10))).toBe(false);
  expect(guard.moved(at(10, 11))).toBe(true);
  expect(guard.moved(at(10, 11))).toBe(false);
  expect(guard.moved(at(12, 11))).toBe(true);
});

it("accepts the first hover of a menu opened with the mouse", () => {
  const guard = new HoverGuard(true);
  expect(guard.moved(at(10, 10))).toBe(true);
  expect(guard.moved(at(10, 10))).toBe(false);
  guard.reset(false);
  expect(guard.moved(at(10, 10))).toBe(false);
});
