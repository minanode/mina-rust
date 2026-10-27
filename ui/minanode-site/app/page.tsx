import { SiteFooter } from "@/components/site-footer";
import { SiteHeader } from "@/components/site-header";
import { Builds } from "@/components/sections/builds";
import { GetStarted } from "@/components/sections/get-started";
import { Hero } from "@/components/sections/hero";
import { Roles } from "@/components/sections/roles";

export default function Home() {
  return (
    <>
      <SiteHeader />
      <main id="main" tabIndex={-1} className="outline-none">
        <Hero />
        <Roles />
        <Builds />
        <GetStarted />
      </main>
      <SiteFooter />
    </>
  );
}
