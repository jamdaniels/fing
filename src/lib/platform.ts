/** Fing runs on macOS and Windows; everything that isn't macOS is Windows. */
export const IS_MAC =
  typeof navigator !== "undefined" && navigator.userAgent.includes("Mac");
