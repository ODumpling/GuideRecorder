import { expect, it } from "vitest";
import { safeNext } from "./redirect";

it.each([
  [undefined, "/dashboard"],
  ["/invitations/abc", "/invitations/abc"],
  [["/a", "/b"], "/a"],
  ["https://evil.example", "/dashboard"],
  ["//evil.example", "/dashboard"],
  ["/\\evil.example", "/dashboard"],
])("safeNext(%j) is %j", (input, expected) => {
  expect(safeNext(input)).toBe(expected);
});
