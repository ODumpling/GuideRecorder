import type { ObjectStore } from "@/lib/storage";

/** An in-memory store; `put` stands in for a client's presigned upload. */
export function memoryStore() {
  const objects = new Map<string, { size: number; contentType: string }>();
  const store: ObjectStore = {
    async presignPut(key, object) {
      return { url: `https://s3.test/${key}?signed`, method: "PUT", headers: { "content-type": object.contentType } };
    },
    async presignGet(key) {
      return `https://s3.test/${key}?get`;
    },
    async head(key) {
      return objects.get(key) ?? null;
    },
    async delete(key) {
      objects.delete(key);
    },
  };
  return { store, objects, put: (key: string, size: number, contentType = "image/webp") => objects.set(key, { size, contentType }) };
}
