import { execFile } from "node:child_process";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);
const MEMORY_FILE = "MEMORY.md"; // OpenClaw's DEFAULT_MEMORY_FILENAME

const TIMEOUT_MS = 10_000;

const handler = async (event: any) => {
    if (event.type !== "agent" || event.action !== "bootstrap") return;
    const files = event.context?.bootstrapFiles;
    if (!Array.isArray(files)) return;

    let summary = "";
    try {
        const { stdout } = await execFileAsync(
            "memoryhub-mcp",
            ["recall", "--agent", "openclaw", "--scope", "global"],
            { timeout: TIMEOUT_MS },
        );
        summary = (stdout ?? "").trim();
    } catch {
        return; // binary missing or recall failed -> inject nothing
    }
    if (!summary) return;

    const block = `\n\n## MemoryHub summary\n\n${summary}\n`;
    const mem = files.find((f: any) => f?.name === MEMORY_FILE);
    if (mem) {
        mem.content = (mem.content ?? "") + block;
        mem.missing = false;
    } else {
        files.push({
            name: MEMORY_FILE,
            path: MEMORY_FILE,
            content: block,
            missing: false,
        });
    }
};

export default handler;
