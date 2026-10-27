import { renderOgImage } from "@/lib/og";

// Written to out/og.png at build time.
export const dynamic = "force-static";

export function GET() {
  return renderOgImage();
}
