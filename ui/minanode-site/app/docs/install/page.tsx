import type { Metadata } from "next";

import { CodeBlock } from "@minanode/design-system/patterns/code-block";
import { KeyValueList } from "@minanode/design-system/patterns/key-value-list";

import { DocsPage } from "@/components/docs/docs-page";
import {
  C,
  DocLink,
  DocSection,
  Prose,
  claimedRows,
} from "@/components/docs/prose";
import { install } from "@/content/docs";
import {
  archiveName,
  release,
  releaseSignerUrl,
  releaseSigningFingerprint,
  verifyReleaseCommand,
} from "@/content/release";

export const metadata: Metadata = {
  title: "Install",
  description: `Download minanode ${release.version} for Linux, verify it, and install it.`,
  alternates: { canonical: "/docs/install/" },
};

const download = `url=${release.downloadUrl}
curl -fLO "$url/${archiveName("x86_64")}"
curl --fail-early -fLO "$url/SHA256SUMS" \\
  -O "$url/SHA256SUMS.sig" -O ${releaseSignerUrl}`;

export default function InstallPage() {
  return (
    <DocsPage
      slug="install"
      title="Install"
      lead="Download the release archive, verify it, and install the mina binary in ~/.local/bin."
    >
      <DocSection id="download" title="1. Download">
        <Prose>
          <p>
            The archives are for Linux. On macOS or Windows,{" "}
            <DocLink href="/docs/docker/">run the node in Docker</DocLink>{" "}
            instead. On Apple silicon, <C>uname -m</C> also prints <C>arm64</C>,
            but the archive does not run on macOS.
          </p>
          <p>
            The release has one archive for each CPU architecture, as the{" "}
            <DocLink href={release.notesUrl}>release notes</DocLink> list. To
            find yours, run <C>uname -m</C>:
          </p>
        </Prose>
        <KeyValueList title="Archives" rows={claimedRows(install.archives)} />
        <Prose>
          <p>
            Download your archive, the checksum list, its signature, and the
            release signer file. For ARM64, replace <C>x86_64</C> with{" "}
            <C>arm64</C>.
          </p>
        </Prose>
        <CodeBlock code={download} label="download commands" />
      </DocSection>

      <DocSection id="check" title="2. Verify the signature and the checksum">
        <Prose>
          <p>
            First verify the checksum list against the release key. Then check
            the archive against the list. Verification needs OpenSSH 8.2 or
            newer.
          </p>
          <p data-claim="C81">
            The release key&apos;s SHA-256 fingerprint is{" "}
            <C>{releaseSigningFingerprint}</C>.
          </p>
          <p>
            The first command prints the signer file&apos;s fingerprint. Compare
            it with the value above. If the key is new or has changed, confirm
            it through a separate trusted channel before you trust it.
          </p>
        </Prose>
        <CodeBlock
          code="ssh-keygen -lf allowed_signers -E sha256"
          label="release key fingerprint"
        />
        <CodeBlock
          code={`${verifyReleaseCommand} &&
sha256sum --check --ignore-missing SHA256SUMS`}
          label="signature and checksum commands"
        />
        <Prose>
          <p>
            The output starts with{" "}
            <C>Good &quot;file&quot; signature for release@minanode.org</C>,
            then gives the archive&apos;s name followed by <C>OK</C>. If
            verification fails, stop. If the checksum says <C>FAILED</C>, delete
            the archive and download it again.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="unpack" title="3. Unpack">
        <CodeBlock
          code={`mkdir mina-${release.version}
tar -xzf ${archiveName("x86_64")} -C mina-${release.version}`}
          label="unpack commands"
        />
        <Prose>
          <p>
            The directory contains <C>mina</C>, <C>README.txt</C>,{" "}
            <C>build-info.json</C>, and the licences.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="run" title="4. Install and check the binary">
        <Prose>
          <p>
            Install <C>mina</C>, then check it. {install.pathNote}
          </p>
        </Prose>
        <CodeBlock
          code={`mkdir -p ~/.local/bin &&
  install -m 755 mina-${release.version}/mina ~/.local/bin/mina
~/.local/bin/mina --version
~/.local/bin/mina build-info`}
          label="install commands"
        />
        <Prose>
          <p>
            <C>--version</C> prints one line with <C>mina {release.version}</C>{" "}
            and the WebRTC backend. <C>build-info</C> prints the version, build
            time, commit, Rust version, and WebRTC backend, one per line.
          </p>
          <p>
            If <C>mina --version</C> without the path prints something else,
            another <C>mina</C> comes first on PATH.{" "}
            <DocLink href="/docs/run-a-node/#troubleshooting">
              Troubleshooting
            </DocLink>{" "}
            shows how to fix it.
          </p>
          <p>
            Next, <DocLink href="/docs/run-a-node/">run a node</DocLink>.
          </p>
        </Prose>
      </DocSection>
      <DocSection id="upgrading" title="Upgrade">
        <Prose>
          <ol>
            <li>Download, verify, and unpack the new archive, as above.</li>
            <li>Stop the node.</li>
            <li>
              Keep a copy of the previous binary, then install the new one.
            </li>
            <li>Start the node with the same work directory and flags.</li>
          </ol>
          <p>For the user service, these commands do steps 2 to 4:</p>
        </Prose>
        <CodeBlock
          code={`systemctl --user stop minanode.service
cp ~/.local/bin/mina ~/.local/bin/mina.previous
install -m 755 mina-${release.version}/mina ~/.local/bin/mina
systemctl --user start minanode.service`}
          label="replace the binary"
        />
        <Prose>
          <p>To go back to the previous binary:</p>
        </Prose>
        <CodeBlock
          code={`systemctl --user stop minanode.service
install -m 755 ~/.local/bin/mina.previous ~/.local/bin/mina
systemctl --user start minanode.service`}
          label="restore the previous binary"
        />
        <Prose>
          <p>
            If the node runs in a terminal, stop it with Ctrl-C instead of{" "}
            <C>systemctl</C>, and start it again the same way. Before you go
            back across versions, check the release notes for changes to the
            data format.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="uninstalling" title="Uninstall">
        <Prose>
          <p>
            Stop the node first. If you installed the user service, remove it:
          </p>
        </Prose>
        <CodeBlock
          code={`systemctl --user disable --now minanode.service
rm ~/.config/systemd/user/minanode.service
systemctl --user daemon-reload`}
          label="remove the user service"
        />
        <Prose>
          <p>Remove the binary that this page installed:</p>
        </Prose>
        <CodeBlock
          code={`rm ~/.local/bin/mina
rm -f ~/.local/bin/mina.previous`}
          label="remove the installed binary"
        />
        <Prose>
          <p>
            These commands keep the work directory. Remove <C>~/.mina</C>, or
            the directory you chose, only if you no longer need its data, logs,
            and saved worker settings. If you put keys there, back them up
            first. If other user services rely on linger, leave it enabled.
          </p>
        </Prose>
      </DocSection>
    </DocsPage>
  );
}
