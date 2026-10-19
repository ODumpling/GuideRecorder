import { createFileRoute } from "@tanstack/react-router";
import { auth } from "@/lib/auth";

const handler = ({ request }: { request: Request }) => auth().handler(request);

export const Route = createFileRoute("/api/auth/$")({
  server: { handlers: { GET: handler, POST: handler } },
});
