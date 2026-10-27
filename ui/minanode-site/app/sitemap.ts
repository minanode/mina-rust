import type { MetadataRoute } from "next";

import { docsPages } from "@/content/docs";
import { siteOrigin } from "@/lib/origin";

export const dynamic = "force-static";

export default function sitemap(): MetadataRoute.Sitemap {
  return [
    { url: `${siteOrigin}/`, changeFrequency: "weekly" },
    { url: `${siteOrigin}/docs/`, changeFrequency: "weekly" },
    { url: `${siteOrigin}/licenses/`, changeFrequency: "yearly" },
    ...docsPages.map(page => ({
      url: `${siteOrigin}/docs/${page.slug}/`,
      changeFrequency: "weekly" as const,
    })),
  ];
}
