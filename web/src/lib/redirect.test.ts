import { expect, it } from "vitest";
import { safeNext } from "./redirect";

it.each([
  [undefined, "/library"],
  ["/invitations/abc", "/invitations/abc"],
  [["/a", "/b"], "/a"],
  ["https://evil.example", "/library"],
  ["//evil.example", "/library"],
  ["/\\evil.example", "/library"],
])("safeNext(%j) is %j", (input, expected) => {
  expect(safeNext(input)).toBe(expected);
});
