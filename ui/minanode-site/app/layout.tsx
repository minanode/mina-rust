import type { Metadata, Viewport } from "next";

import { themeInitScript } from "@minanode/design-system/lib/theme";

import { ogAlt, ogSize } from "@/lib/og";
import { isPreview, siteOrigin } from "@/lib/origin";

import "./globals.css";

const title = "minanode: a second Mina client, in Rust";
const description =
  "A community-led second Mina client: a native node in Rust and a full node in the browser, checked against the OCaml node by replaying the chain, checking proofs and comparing fork choice.";

export const metadata: Metadata = {
  metadataBase: new URL(siteOrigin),
  ...(isPreview ? { robots: { index: false, follow: false } } : {}),
  title: { default: title, template: "%s · minanode" },
  description,
  applicationName: "minanode",
  openGraph: {
    type: "website",
    siteName: "minanode",
    url: "/",
    title,
    description,
    locale: "en_GB",
    images: [{ url: "/og.png", ...ogSize, alt: ogAlt, type: "image/png" }],
  },
  twitter: {
    card: "summary_large_image",
    title,
    description,
    images: [{ url: "/og.png", alt: ogAlt }],
  },
  icons: {
    icon: [{ url: "/icon.svg", type: "image/svg+xml" }],
    apple: [{ url: "/apple-touch-icon.png", sizes: "180x180" }],
  },
  alternates: { canonical: "/" },
  formatDetection: { telephone: false, address: false, email: false },
};

export const viewport: Viewport = {
  themeColor: "#0a0f14",
  colorScheme: "dark light",
};

export default function RootLayout({
  children,
}: Readonly<{ children: React.ReactNode }>) {
  return (
    <html lang="en" className="dark" suppressHydrationWarning>
      <head>
        <script dangerouslySetInnerHTML={{ __html: themeInitScript }} />
      </head>
      <body className="min-h-dvh">
        <a
          href="#main"
          className="sr-only rounded-md bg-primary px-4 py-2 text-sm font-medium text-primary-foreground focus-visible:not-sr-only focus-visible:fixed focus-visible:top-3 focus-visible:left-gutter focus-visible:shadow-lg focus-visible:ring-4 focus-visible:ring-background focus-visible:z-50"
        >
          Skip to content
        </a>
        {children}
      </body>
    </html>
  );
}
