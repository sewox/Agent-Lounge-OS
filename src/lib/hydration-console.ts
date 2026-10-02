/** Detect React hydration noise in console / pageerror text (dev + minified prod). */
export function isHydrationConsoleMessage(text: string): boolean {
  return /hydrat|Minified React error #(418|421|422|423|425)/i.test(text);
}
