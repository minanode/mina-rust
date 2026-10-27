import { Button } from "@minanode/design-system/components/button";
import { SiteHeader } from "@/components/site-header";
import { SiteFooter } from "@/components/site-footer";

export default function NotFound() {
  return (
    <div className="flex min-h-dvh flex-col">
      <SiteHeader />
      <main
        id="main"
        tabIndex={-1}
        className="mx-auto flex w-full max-w-prose flex-1 flex-col items-start justify-center gap-6 px-gutter py-16 outline-none"
      >
        <h1 className="text-title text-foreground">Page not found</h1>
        <p className="text-muted-foreground">
          Check the web address, or head to the home page or node-runner docs.
        </p>
        <div className="flex flex-wrap gap-3">
          <Button asChild>
            <a href="/docs/">Read the docs</a>
          </Button>
          <Button asChild variant="outline">
            <a href="/">Go to the home page</a>
          </Button>
        </div>
      </main>
      <SiteFooter />
    </div>
  );
}
