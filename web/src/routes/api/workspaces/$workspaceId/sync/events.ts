import { createFileRoute } from "@tanstack/react-router";
import { authorize } from "@/lib/api";
import { onWorkspaceChange } from "@/lib/sync-events";

const HEARTBEAT = 25_000;

/**
 * Server-sent events: a `change` event with the new seq whenever a guide in
 * the workspace changes. A nudge only; fetch `changes` to see what changed.
 */
export const Route = createFileRoute("/api/workspaces/$workspaceId/sync/events")({
  server: {
    handlers: {
      GET: async ({ request, params: { workspaceId } }) => {
        const access = await authorize(request, workspaceId, { guide: ["read"] }, "sync");
        if ("error" in access) return access.error;

        const encoder = new TextEncoder();
        let cleanup = () => {};
        const stream = new ReadableStream<Uint8Array>({
          async start(controller) {
            const send = (text: string) => controller.enqueue(encoder.encode(text));
            const unsubscribe = await onWorkspaceChange(workspaceId, (seq) => send(`event: change\ndata: ${JSON.stringify({ seq })}\n\n`));
            const heartbeat = setInterval(() => send(": ping\n\n"), HEARTBEAT);
            cleanup = () => {
              clearInterval(heartbeat);
              unsubscribe();
            };
            request.signal.addEventListener("abort", () => {
              cleanup();
              controller.close();
            });
            send("retry: 5000\n\n");
          },
          cancel: () => cleanup(),
        });
        return new Response(stream, {
          headers: { "content-type": "text/event-stream", "cache-control": "no-store", connection: "keep-alive" },
        });
      },
    },
  },
});
