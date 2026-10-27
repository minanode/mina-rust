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
  description:
    "Download minanode 0.20.0 for Linux, check its SHA-256, unpack it and run it.",
  alternates: { canonical: "/docs/install/" },
};

const download = `url=${release.downloadUrl}
curl -fLO "$url/${archiveName("x86_64")}"
curl --fail-early -fLO "$url/SHA256SUMS" \\
  -O "$url/SHA256SUMS.sig" -O ${releaseSignerUrl}`;

export default function InstallPage() {
  return (
    <DocsPage slug="install" title="Install" lead={null}>
      <DocSection id="download" title="1. Download">
        <Prose>
          <p>
            The archives are for Linux. On macOS or Windows,{" "}
            <DocLink href="/docs/docker/">run the node in Docker</DocLink>.
            macOS on Apple silicon also says <C>arm64</C> to <C>uname -m</C>,
            but the archive will not run there.
          </p>
          <p>
            The release has one archive for each CPU architecture (see the{" "}
            <DocLink href={release.notesUrl}>release notes</DocLink>). Check
            which you have:
          </p>
        </Prose>
        <KeyValueList title="Archives" rows={claimedRows(install.archives)} />
        <Prose>
          <p>
            Download yours, the checksum list, its signature and the release
            signer. For ARM64, put <C>arm64</C> in place of <C>x86_64</C>.
          </p>
        </Prose>
        <CodeBlock code={download} label="download commands" />
      </DocSection>

      <DocSection id="check" title="2. Verify and check the SHA-256">
        <Prose>
          <p>
            Verify the checksum list against the release key before checking the
            archive. Verification needs OpenSSH 8.2 or newer.
          </p>
          <p data-claim="C81">
            Release key fingerprint (SHA-256):{" "}
            <C>{releaseSigningFingerprint}</C>.
          </p>
          <p>
            Compare the signer file&apos;s fingerprint with the value above.
            Before trusting a new or changed key, confirm it through a separate
            trusted channel.
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
            Expect a good signature for <C>release@minanode.org</C>, then the
            archive&apos;s name and <C>OK</C>. If verification fails, stop. If
            the checksum says <C>FAILED</C>, delete the archive and download it
            again.
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
            The directory holds the program, <C>mina</C>, with a{" "}
            <C>README.txt</C>, its build details in <C>build-info.json</C>, and
            the licences.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="run" title="4. Run it">
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
            <C>--version</C> prints one line, <C>mina {release.version}</C> and
            the WebRTC backend. <C>build-info</C> prints the version, build
            time, commit, Rust version and WebRTC backend, one per line. Next:{" "}
            <DocLink href="/docs/run-a-node/">run a node</DocLink>.
          </p>
        </Prose>
      </DocSection>
      <DocSection id="upgrading" title="Upgrading">
        <Prose>
          <p>
            Download and check the new archive as above. Stop the node, keep the
            previous binary, then replace it. Keep the work directory and start
            with the same flags. For the user service, use these commands:
          </p>
        </Prose>
        <CodeBlock
          code={`systemctl --user stop mina.service
cp ~/.local/bin/mina ~/.local/bin/mina.previous
install -m 755 mina-${release.version}/mina ~/.local/bin/mina
systemctl --user start mina.service`}
          label="replace the binary"
        />
        <Prose>
          <p>To roll back this binary replacement:</p>
        </Prose>
        <CodeBlock
          code={`systemctl --user stop mina.service
install -m 755 ~/.local/bin/mina.previous ~/.local/bin/mina
systemctl --user start mina.service`}
          label="restore the previous binary"
        />
        <Prose>
          <p>
            For a node started in a terminal, stop it with Ctrl-C instead of{" "}
            <C>systemctl</C>, and start it again the same way. Check future
            release notes for data-format changes before rolling back across
            versions.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="uninstalling" title="Uninstalling">
        <Prose>
          <p>
            Stop the node first. If you installed the user service, remove it:
          </p>
        </Prose>
        <CodeBlock
          code={`systemctl --user disable --now mina.service
rm ~/.config/systemd/user/mina.service
systemctl --user daemon-reload`}
          label="remove the user service"
        />
        <Prose>
          <p>Remove the binary installed by this guide:</p>
        </Prose>
        <CodeBlock
          code={`rm ~/.local/bin/mina
rm -f ~/.local/bin/mina.previous`}
          label="remove the installed binary"
        />
        <Prose>
          <p>
            These commands keep the work directory. Remove <C>~/.mina</C>, or
            your chosen directory, only if you no longer need its data, logs and
            saved worker settings. Back up any keys you put there first. Leave
            linger enabled if other user services rely on it.
          </p>
        </Prose>
      </DocSection>
    </DocsPage>
  );
}
