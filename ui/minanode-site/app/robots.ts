import type { MetadataRoute } from "next";

import { isPreview, siteOrigin } from "@/lib/origin";

export const dynamic = "force-static";

export default function robots(): MetadataRoute.Robots {
  return {
    rules: {
      userAgent: "*",
      ...(isPreview ? { disallow: "/" } : { allow: "/" }),
    },
    sitemap: `${siteOrigin}/sitemap.xml`,
  };
}
