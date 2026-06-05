import { existsSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";

export type UploadItem = { project?: string; filename: string; path: string };

/** Map the *.md daily notes in a memory dir to upload items (project omitted -> server _default). */
export function collectItems(memoryDir: string): UploadItem[] {
    if (!existsSync(memoryDir)) return [];
    const items: UploadItem[] = [];
    for (const name of readdirSync(memoryDir).sort()) {
        if (!name.endsWith(".md")) continue;
        const path = join(memoryDir, name);
        if (!statSync(path).isFile()) continue;
        items.push({ filename: name, path });
    }
    return items;
}
