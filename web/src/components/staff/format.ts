export const day = (d: Date | string) => new Date(d).toLocaleDateString(undefined, { dateStyle: "medium" });
export const dateTime = (d: Date | string) => new Date(d).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });

/** `YYYY-MM-DD` (UTC), `days` from now, for date inputs. */
export const isoDay = (days: number) => new Date(Date.now() + days * 24 * 60 * 60 * 1000).toISOString().slice(0, 10);
