import { describe, expect, it } from "vitest";
import { linuxHome } from "./path";

describe("linuxHome", () => {
  it("finds the Linux home inside a WSL distribution", () => {
    expect(linuxHome("\\\\wsl.localhost\\Ubuntu\\home\\matej\\src\\app")).toBe("\\\\wsl.localhost\\Ubuntu\\home\\matej");
    expect(linuxHome("\\\\wsl.localhost\\Ubuntu\\home\\matej")).toBe("\\\\wsl.localhost\\Ubuntu\\home\\matej");
    expect(linuxHome("\\\\wsl$\\Debian\\root\\x")).toBe("\\\\wsl$\\Debian\\root");
    expect(linuxHome("//wsl.localhost/Ubuntu/home/u/a")).toBe("\\\\wsl.localhost\\Ubuntu\\home\\u");
  });
  it("uses /home outside a home folder", () => {
    expect(linuxHome("\\\\wsl.localhost\\Ubuntu\\etc")).toBe("\\\\wsl.localhost\\Ubuntu\\home");
    expect(linuxHome("\\\\wsl.localhost\\Ubuntu")).toBe("\\\\wsl.localhost\\Ubuntu\\home");
  });
  it("is null outside WSL", () => {
    expect(linuxHome("C:\\Users\\matej")).toBeNull();
    expect(linuxHome("\\\\nas\\share\\home\\u")).toBeNull();
    expect(linuxHome("/home/u")).toBeNull();
  });
});
