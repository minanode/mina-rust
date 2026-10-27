import type { Metadata } from "next";

import { CodeBlock } from "@minanode/design-system/patterns/code-block";

import { DocsPage } from "@/components/docs/docs-page";
import { C, DocLink, DocSection, Prose } from "@/components/docs/prose";
import { release } from "@/content/release";
import { sourceBuild } from "@/content/docs";

export const metadata: Metadata = {
  title: "Build from source",
  description: "Build the minanode dashboard and native node from source.",
  alternates: { canonical: "/docs/build-from-source/" },
};

const packages = `sudo apt update
sudo apt install build-essential pkg-config libssl-dev clang cmake \\
  protobuf-compiler git curl ca-certificates python3 xz-utils`;

const toolchains = `# x86-64: install rustup without changing your shell configuration.
curl -fL -o rustup-init \\
  https://static.rust-lang.org/rustup/dist/x86_64-unknown-linux-gnu/rustup-init
chmod u+x rustup-init
./rustup-init -y --no-modify-path \\
  --profile minimal --default-toolchain none
export PATH="$HOME/.cargo/bin:$PATH"
rustup toolchain install 1.92 --profile minimal --no-self-update
rustup component add rustfmt --toolchain 1.92

# Install Node.js 24.11.0 with npm into ~/.local/node (x86-64 example).
mkdir -p ~/.local/node
curl -fLO https://nodejs.org/dist/v24.11.0/node-v24.11.0-linux-x64.tar.xz
tar -xJf node-v24.11.0-linux-x64.tar.xz \\
  -C ~/.local/node --strip-components=1
export PATH="$HOME/.local/node/bin:$HOME/.cargo/bin:$PATH"`;

const build = `git clone ${release.repositoryUrl} mina-rust
cd mina-rust
cd ui && npm ci && npm run build:dashboard && cd ..
CARGO_BUILD_JOBS=8 cargo build --release --locked \\
  -p mina-cli --bin mina`;

export default function BuildFromSourcePage() {
  return (
    <DocsPage
      slug="build-from-source"
      title="Build from source"
      lead="Build the dashboard first, then the node."
    >
      <DocSection id="prepare" title="1. Prepare">
        <Prose>
          <p>
            On Ubuntu, ask an administrator to install the build packages. The
            commands below are for Ubuntu on x86-64. On ARM64, use rustup&apos;s
            aarch64 installer and the Node.js arm64 archive.
          </p>
        </Prose>
        <CodeBlock code={packages} label="system packages" />
        <Prose>
          <p data-claim={sourceBuild.versions.claim}>
            {sourceBuild.versions.text}
          </p>
        </Prose>
        <CodeBlock code={toolchains} label="user-level toolchains" />
      </DocSection>

      <DocSection id="build" title="2. Build">
        <Prose>
          <p>Run these commands in one shell.</p>
        </Prose>
        <CodeBlock code={build} label="build commands" />
        <Prose>
          <p>
            Cargo writes the binary to <C>target/release/mina</C>. If the
            dashboard was not built first, the binary has no dashboard, and the
            release build warns.
          </p>
          <p data-claim={sourceBuild.circuits.claim}>
            {sourceBuild.circuits.text}
          </p>
          <p data-claim={sourceBuild.measurements.claim}>
            {sourceBuild.measurements.text}
          </p>
          <p>
            Node.js 24.11.0 prints engine warnings for some test dependencies
            that ask for a newer patch version. The warnings do not stop the
            dashboard build.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="webrtc-backend" title="WebRTC backend">
        <Prose>
          <p>
            The default <C>p2p-webrtc</C> feature uses rustrtc for server-side
            WebRTC. To build with the previous webrtc-rs backend, enable{" "}
            <C>p2p-webrtc-rs</C> with the command below. The backend is selected
            at build time; there is no automatic fallback.
          </p>
        </Prose>
        <CodeBlock
          code={`CARGO_BUILD_JOBS=8 cargo build --release --locked \\
  -p mina-cli --bin mina --features p2p-webrtc-rs`}
          label="build with webrtc-rs"
        />
        <Prose>
          <p>
            <C>mina --version</C> and <C>mina build-info</C> name the selected
            backend.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="verify" title="3. Verify">
        <CodeBlock
          code={`target/release/mina --version
target/release/mina build-info`}
          label="check the binary"
        />
        <Prose>
          <p>
            Both commands should report the build you expect. Then{" "}
            <DocLink href="/docs/run-a-node/">run a node</DocLink> and open its
            dashboard.
          </p>
          <p data-claim={sourceBuild.release.claim}>
            {sourceBuild.release.text}
          </p>
        </Prose>
      </DocSection>
    </DocsPage>
  );
}
