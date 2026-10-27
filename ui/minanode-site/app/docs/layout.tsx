import { SiteFooter } from "@/components/site-footer";
import { SiteHeader } from "@/components/site-header";

export default function DocsLayout({
  children,
}: Readonly<{ children: React.ReactNode }>) {
  return (
    <>
      <SiteHeader />
      <main id="main" tabIndex={-1} className="outline-none">
        {children}
      </main>
      <SiteFooter homeHref="/" />
    </>
  );
}
