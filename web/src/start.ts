import { createCsrfMiddleware, createStart } from "@tanstack/react-start";

// Server functions only accept same-origin requests. Better Auth checks the
// origin of its own endpoints; the asset API needs a session cookie or token.
export const startInstance = createStart(() => ({
  requestMiddleware: [createCsrfMiddleware({ filter: (ctx) => ctx.handlerType === "serverFn" })],
}));
