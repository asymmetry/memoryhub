import { describe, it, expect } from "vitest";
import { mkdtempSync, mkdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { collectItems } from "../lib/items.ts";

describe("collectItems", () => {
  it("maps daily-note markdown files to upload items", () => {
    const root = mkdtempSync(join(tmpdir(), "ocmem-"));
    const memory = join(root, "memory");
    mkdirSync(memory);
    writeFileSync(join(memory, "2026-06-04.md"), "note");
    writeFileSync(join(memory, "2026-06-03-vendor.md"), "note2");
    writeFileSync(join(memory, "notes.txt"), "ignore");

    const items = collectItems(memory);
    expect(items.map((i) => i.filename).sort()).toEqual([
      "2026-06-03-vendor.md",
      "2026-06-04.md",
    ]);
    expect(items[0].path).toContain(memory);
    expect(items[0].project).toBeUndefined();
  });

  it("returns [] when the dir is missing", () => {
    expect(collectItems(join(tmpdir(), "definitely-missing-xyz"))).toEqual([]);
  });
});
