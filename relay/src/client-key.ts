/** The client's IPv4 address, or its IPv6 /64: one host can use a whole /64. */
export function clientKey(ip: string | null): string {
  if (!ip) return "unknown";
  if (!ip.includes(":")) return ip;
  const [head, tail = ""] = ip.split("::");
  const left = head ? head.split(":") : [];
  const right = tail ? tail.split(":") : [];
  const full = ip.includes("::") ? [...left, ...Array(8 - left.length - right.length).fill("0"), ...right] : left;
  return full.slice(0, 4).map((g) => parseInt(g, 16).toString(16)).join(":") + "::/64";
}
