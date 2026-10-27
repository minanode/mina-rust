import type { Metadata } from "next";
import { SiteHeader } from "@/components/site-header";
import { SiteFooter } from "@/components/site-footer";
import { Container } from "@/components/container";
import { DocLink, DocSection, Prose } from "@/components/docs/prose";
import { release } from "@/content/release";

export const metadata: Metadata = {
  title: "Licenses",
  description: "Software and font licenses for minanode.",
  alternates: { canonical: "/licenses/" },
};

export default function LicensesPage() {
  return (
    <>
      <SiteHeader />
      <main id="main" tabIndex={-1} className="outline-none">
        <Container className="py-section">
          <article className="flex max-w-prose flex-col gap-10">
            <h1 className="text-title">Licenses</h1>
            <DocSection id="software" title="Software">
              <Prose className="rounded-xl border bg-card p-6">
                <p>
                  The node source is licensed under Apache 2.0. See the{" "}
                  <DocLink href={`${release.repositoryUrl}/blob/main/LICENSE`}>
                    software license
                  </DocLink>{" "}
                  and the license notices included in the release archive.
                </p>
              </Prose>
            </DocSection>
            <DocSection id="fonts" title="Fonts">
              <Prose className="rounded-xl border bg-card p-6">
                <p>
                  This site uses Inter and JetBrains Mono under the SIL Open
                  Font License 1.1.
                </p>
                <p>
                  <DocLink href="/licenses/fonts.txt">
                    Read the font licenses and copyright notices
                  </DocLink>
                  .
                </p>
              </Prose>
            </DocSection>
          </article>
        </Container>
      </main>
      <SiteFooter />
    </>
  );
}
