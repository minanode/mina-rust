// The iOS version in an iPhone, iPad or iPod user agent ("CPU iPhone OS
// 17_4_1 like Mac OS X" gives [17, 4, 1]), or null for any other agent. Reads
// the user agent rather than navigator.platform, which stays the host's
// platform when the agent is spoofed or emulated.
export function iOSversion(
  userAgent: string = typeof navigator === 'undefined'
    ? ''
    : navigator.userAgent,
): number[] | null {
  if (!/iP(hone|od|ad)/.test(userAgent)) {
    return null;
  }
  const v = userAgent.match(/OS (\d+)_(\d+)(?:_(\d+))?/);
  if (!v) {
    return null;
  }
  return [parseInt(v[1], 10), parseInt(v[2], 10), parseInt(v[3] || '0', 10)];
}
