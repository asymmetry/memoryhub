import { spawn } from "node:child_process";
import { join } from "node:path";
import { collectItems } from "../../lib/items.ts";

const handler = async (event: any) => {
    if (
        event.type !== "command" ||
        (event.action !== "new" && event.action !== "reset")
    ) {
        return;
    }
    const workspaceDir: string | undefined = event.context?.workspaceDir;
    if (!workspaceDir) return;

    const items = collectItems(join(workspaceDir, "memory"));
    if (items.length === 0) return;

    // Fire-and-forget so /new and /reset stay fast; never throw (it must not break the session).
    try {
        const child = spawn(
            "memoryhub-mcp",
            ["upload", "--agent", "openclaw"],
            {
                stdio: ["pipe", "ignore", "ignore"],
            },
        );
        child.on("error", () => {}); // swallow a missing binary
        child.stdin.end(JSON.stringify(items));
    } catch {
        // ignore
    }
};

export default handler;
