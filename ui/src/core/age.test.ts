import { describe, expect, it } from "vitest";
import { age, ageTier, ageWords } from "./age";

const MIN = 60_000, HOUR = 60 * MIN, DAY = 24 * HOUR;
const now = Date.UTC(2026, 10, 1);

describe("age labels", () => {
  it("uses ' h d M Y so months never read as minutes", () => {
    expect(age(now - 5 * MIN, now).label).toBe("5'");
    expect(age(now - (3 * HOUR + 12 * MIN), now).label).toBe("3h12'");
    expect(age(now - 29 * DAY, now).label).toBe("29d");
    expect(age(now - 45 * DAY, now).label).toBe("1M");
    expect(age(now - 800 * DAY, now).label).toBe("2Y");
    expect(ageWords(now - 23 * MIN, now)).toBe("23 minutes");
    expect(ageWords(now - (22 * HOUR + 13 * MIN), now)).toBe("22 hours 13 minutes");
    expect(ageWords(now - 3 * HOUR, now)).toBe("3 hours");
    expect(ageWords(now - DAY, now)).toBe("1 day");
    expect(ageWords(now - 800 * DAY, now)).toBe("2 years");
  });
  it("tiers are monotonic and older files are neutral", () => {
    const ds = [MIN, 2 * HOUR, 3 * DAY, 20 * DAY, 200 * DAY, 900 * DAY];
    const t = ds.map((d) => ageTier(now - d, now));
    expect(t).toEqual([0, 1, 2, 3, 4, 5]);
  });
  it("future timestamps clamp to newest tier", () => {
    expect(ageTier(now + DAY, now)).toBe(0);
  });
});
