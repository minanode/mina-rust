import { renderAppleIcon } from "@/lib/og";

// Written to out/apple-touch-icon.png at build time.
export const dynamic = "force-static";

export function GET() {
  return renderAppleIcon();
}
