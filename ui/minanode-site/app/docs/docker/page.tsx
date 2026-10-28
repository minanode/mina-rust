import type { Metadata } from "next";

import { CodeBlock } from "@minanode/design-system/patterns/code-block";

import { DocsPage } from "@/components/docs/docs-page";
import {
  C,
  Caution,
  DocLink,
  DocSection,
  Prose,
  Terms,
} from "@/components/docs/prose";
import {
  composeBaseUrl,
  dockerImage,
  dockerImagePinned,
  dockerImageRef,
  release,
} from "@/content/release";
import { readRepoFile } from "@/lib/repo-file";

export const metadata: Metadata = {
  title: "Run with Docker",
  description:
    "Run minanode on Mina's devnet with Docker or Docker Compose: ports, the work directory, logs, upgrades, the SNARK worker and block production.",
  alternates: { canonical: "/docs/docker/" },
};

const image = dockerImageRef;

// The compose files are shown as they are in the repository, and served
// from the site at composeBaseUrl.
const compose = readRepoFile("docker-compose.yml");
const composeProducer = readRepoFile("docker-compose.block-producer.yml");
const composeRelay = readRepoFile("docker-compose.relay.yml");
// The relay section returns with the run-a-relay page, at the web node launch.
const showRelay = false;
if (!compose.includes(`image: \${MINA_IMAGE:-${image}}`))
  throw new Error(`docker-compose.yml does not default to ${image}`);

// Docker's own copy of the log, limited as in the compose file.
const logLimits = "--log-opt max-size=10m --log-opt max-file=3";

const run = `mkdir -p ~/mina-state
docker run -d --name mina --restart unless-stopped \\
  --user "$(id -u):$(id -g)" \\
  -p 127.0.0.1:3000:3000 -p 8302:8302 \\
  -v ~/mina-state:/data \\
  ${logLimits} \\
  ${image}
docker logs -f mina`;

const runSecond = `mkdir -p ~/mina-state-2
docker run -d --name mina2 --restart unless-stopped \\
  --user "$(id -u):$(id -g)" \\
  -p 127.0.0.1:3010:3000 -p 8312:8312 -e MINA_LIBP2P_PORT=8312 \\
  -v ~/mina-state-2:/data \\
  ${logLimits} \\
  ${image}
docker logs -f mina2`;

const repository = release.repositoryUrl.replace("https://github.com/", "");

const pull = `docker pull ${image}
# The Digest line must match the release notes:
#   Digest: ${release.dockerDigest}`;

const attest = `gh attestation verify oci://${image} -R ${repository}`;

const composeStart = `mkdir -p minanode && cd minanode
curl -fLO ${composeBaseUrl}/docker-compose.yml
mkdir -p mina-state
printf 'UID=%s\\nGID=%s\\n' "$(id -u)" "$(id -g)" > .env
docker compose up -d
docker compose logs -f`;

const composeSecond = `mkdir -p minanode2 && cd minanode2
curl -fLO ${composeBaseUrl}/docker-compose.yml
mkdir -p mina-state
printf 'UID=%s\\nGID=%s\\n' "$(id -u)" "$(id -g)" > .env
cat >> .env <<'EOF'
COMPOSE_PROJECT_NAME=minanode2
MINA_CONTAINER_NAME=mina2
MINA_DASHBOARD_PORT=3010
MINA_LIBP2P_PORT=8312
EOF
docker compose up -d`;

const options = `docker run -d --name mina --restart unless-stopped \\
  --user "$(id -u):$(id -g)" \\
  -p 127.0.0.1:3000:3000 -p 8302:8302 \\
  -v ~/mina-state:/data -e MINA_VERBOSITY=debug \\
  ${logLimits} \\
  ${image} \\
  node --network devnet --libp2p-external-ip 203.0.113.7`;

const reuse = `systemctl --user stop minanode.service   # or Ctrl-C in its terminal
docker run -d --name mina --restart unless-stopped \\
  --user "$(id -u):$(id -g)" \\
  -p 127.0.0.1:3000:3000 -p 8302:8302 \\
  -v ~/.mina:/data \\
  ${logLimits} \\
  ${image}`;

const report = `docker exec -it mina mina report --no-browser
docker exec mina mina report -m "Sync stopped progressing" \\
  --no-browser --yes
docker exec mina mina report -m "Sync stopped progressing" \\
  --output /data/report.md`;

const upgrade = `docker pull ${dockerImage}:NEW_VERSION
docker stop mina && docker rm mina
# Run the same docker run command as before, with the new tag.`;

const composeUpgrade = `# In .env: MINA_IMAGE=${dockerImage}:NEW_VERSION
docker compose pull
docker compose up -d`;

const keygen = `mkdir -p -m 700 keys
printf "Key password: " && IFS= read -rs MINA_PRIVKEY_PASS && echo &&
  export MINA_PRIVKEY_PASS
docker run --rm --user "$(id -u):$(id -g)" -e MINA_PRIVKEY_PASS \\
  -v "$PWD/keys:/keys" ${image} \\
  wallet generate --output /keys/producer-key`;

const producerEnv = `(umask 077 &&
  printf "MINA_PRIVKEY_PASS='%s'\\n" "$MINA_PRIVKEY_PASS" > producer.env)
unset MINA_PRIVKEY_PASS`;

const producerStart = `curl -fLO ${composeBaseUrl}/docker-compose.block-producer.yml
echo 'COMPOSE_FILE=docker-compose.yml:docker-compose.block-producer.yml' >> .env
docker compose run --rm mina wallet address --from /keys/producer-key
docker compose up -d`;

const relayStart = `curl -fLO ${composeBaseUrl}/docker-compose.relay.yml
cat >> .env <<'EOF'
COMPOSE_FILE=docker-compose.yml:docker-compose.relay.yml
MINA_NODE_ARGS=--libp2p-external-ip 203.0.113.7
EOF
docker compose up -d`;

const build = `git clone ${release.repositoryUrl} mina-rust
cd mina-rust
mkdir release && cd release
url=${release.downloadUrl}
curl -fLO "$url/mina-${release.version}-linux-x86_64.tar.gz" \\
  -O "$url/mina-${release.version}-linux-arm64.tar.gz" \\
  -O "$url/SHA256SUMS" -O "$url/SHA256SUMS.sig"
cd ..
tools/release/docker/build-image.sh ${release.version} release`;

export default function DockerPage() {
  return (
    <DocsPage
      slug="docker"
      title="Run with Docker"
      lead={`The image ${image} holds the same mina program as the Linux release, built from the signed release archives, for x86-64 and ARM64. With Docker Desktop it also runs on macOS and Windows.`}
    >
      <DocSection id="quick-start" title="Quick start">
        <CodeBlock code={run} label="docker run commands" />
        <Prose>
          <p>
            The node starts as a devnet full node: it finds peers, syncs and
            follows the tip. Open the dashboard at <C>http://127.0.0.1:3000</C>.
            It keeps its state in <C>~/mina-state</C>, so a new container picks
            up where the last one stopped. <C>--user</C> runs it as you, so the
            files there stay yours.
          </p>
          <p>
            If a container called <C>mina</C> already exists, or another program
            uses port 3000 or 8302, run this node with its own name, ports and
            state directory, as below. Two nodes must not share a state
            directory (see{" "}
            <DocLink href="/docs/docker/#reuse">
              reuse a native node&apos;s state
            </DocLink>
            ). Then use the new name, here <C>mina2</C>, in the <C>docker</C>{" "}
            commands on this page, and open the dashboard at{" "}
            <C>http://127.0.0.1:3010</C>.
          </p>
        </Prose>
        <CodeBlock
          code={runSecond}
          label="a node with its own name, ports and state"
        />
        <Prose>
          <p>
            The image runs <C>mina</C>: <C>docker exec mina mina</C> runs a
            second <C>mina</C> command in the same container.
          </p>
        </Prose>
        <CodeBlock
          code="docker exec mina mina node status"
          label="status of the running node"
        />
      </DocSection>

      <DocSection id="check-the-image" title="Check the image">
        <Prose>
          <p>
            <C>docker pull</C> prints the image&apos;s digest. Compare it with
            the one in the{" "}
            <DocLink href={release.notesUrl}>release notes</DocLink>; each
            release&apos;s notes list its image&apos;s digest.
          </p>
        </Prose>
        <CodeBlock code={pull} label="pull the image and check its digest" />
        <Prose>
          <p>
            A tag can be moved to another image; a digest can&apos;t. To run
            exactly the image you checked, use <C>{dockerImagePinned}</C> in
            place of <C>{image}</C> in the commands on this page. In Compose,
            set <C>MINA_IMAGE</C> to it in <C>.env</C>.
          </p>
          <p>
            The GitHub CLI checks the image&apos;s build provenance. A passing
            check shows that the repository&apos;s Docker image workflow built
            the image from the signed release archives.
          </p>
        </Prose>
        <CodeBlock code={attest} label="check the build provenance" />
      </DocSection>

      <DocSection id="compose" title="With Docker Compose">
        <Prose>
          <p>
            The compose file runs the same node, with the dashboard on this
            computer only, its SNARK worker controlled from the dashboard, and
            the state in <C>./mina-state</C>. It starts again after a reboot.
          </p>
        </Prose>
        <CodeBlock code={composeStart} label="start with Docker Compose" />
        <Prose>
          <p>
            Settings such as the work directory, the image tag, more node
            options and the ports go in <C>.env</C>; the file lists them. Stop
            the node with <C>docker compose stop</C> and start it again with{" "}
            <C>docker compose start</C>.
          </p>
          <p>
            If you ran the quick start first, stop and remove its container
            with <C>docker stop mina &amp;&amp; docker rm mina</C>. Its state
            stays in <C>~/mina-state</C>: to keep that chain, set{" "}
            <C>MINA_STATE_DIR</C> in <C>.env</C> to its full path, which{" "}
            <C>echo ~/mina-state</C> prints.
          </p>
          <p>
            If a container called <C>mina</C> already exists, or another program
            uses port 3000 or 8302, set this node up in its own directory with
            its own settings in <C>.env</C>, as below. Each directory keeps its
            own <C>mina-state</C>, so the two nodes never share state. Without
            its own <C>COMPOSE_PROJECT_NAME</C>, a second{" "}
            <C>docker compose up -d</C> replaces the first node&apos;s
            container.
          </p>
        </Prose>
        <CodeBlock
          code={composeSecond}
          label="a second node, in its own directory"
        />
        <CodeBlock code={compose} label="docker-compose.yml" />
      </DocSection>

      <DocSection id="options" title="Options and settings">
        <Prose>
          <p>
            Anything after the image name replaces the default command (
            <C>node --network devnet</C>), so give the whole command with your
            options. An option with an environment variable, listed by{" "}
            <C>docker run --rm {image} node --help</C>, may be set with{" "}
            <C>-e</C> instead.
          </p>
        </Prose>
        <CodeBlock code={options} label="docker run with options" />
        <Prose>
          <p>
            The image sets <C>MINA_HOME=/data</C> and <C>HOME=/data</C>, so
            everything the node writes goes to the work directory, and{" "}
            <C>MINA_HTTP_ADDR=0.0.0.0</C>, so the dashboard is reachable through
            the published port. Leave these as they are. In Compose, put more
            options in <C>MINA_NODE_ARGS</C> in <C>.env</C>.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="ports" title="Ports">
        <Terms
          mono
          items={[
            {
              term: "127.0.0.1:3000:3000",
              text: "The dashboard and HTTP API, published on this computer only.",
            },
            {
              term: "8302:8302",
              text: "Peers (TCP), on every network interface on purpose: other nodes must reach it. Open it in your firewall or router to accept incoming peers.",
            },
          ]}
        />
        <Prose>
          <p>
            A plain <C>-p 3000:3000</C> publishes the dashboard and the whole
            HTTP API on every network interface. On Linux, Docker&apos;s port
            rules come before ufw&apos;s, so ufw doesn&apos;t block a published
            port. On a server, keep <C>127.0.0.1:3000:3000</C> and open the
            dashboard through an{" "}
            <DocLink href="/docs/dashboard/#on-a-server">SSH tunnel</DocLink>.
            To use another port on the host, change only the first number, as in{" "}
            <C>127.0.0.1:3010:3000</C>. Worker control works at any port on{" "}
            <C>127.0.0.1</C>.
          </p>
          <p>
            Other listeners, such as <C>--metrics-port</C>, listen on{" "}
            <C>127.0.0.1</C> inside the container by default, which the host
            can&apos;t reach. Set their address option to <C>0.0.0.0</C> and
            publish the port on <C>127.0.0.1</C> as above.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="peers" title="Peers behind NAT">
        <Prose>
          <p>
            Behind a router, forward TCP port 8302 to this machine and give the
            node your public address with{" "}
            <C>--libp2p-external-ip 203.0.113.7</C>. Inside the container, the
            node sees only Docker&apos;s private network and cannot discover
            that address.
          </p>
          <p>
            For another peer port, use the same number on both sides and tell
            the node: <C>-p 8312:8312 -e MINA_LIBP2P_PORT=8312</C>, or{" "}
            <C>MINA_LIBP2P_PORT=8312</C> in Compose&apos;s <C>.env</C>.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="state" title="The work directory">
        <Prose>
          <p>
            The directory mounted on <C>/data</C> is the node&apos;s work
            directory, as <C>~/.mina</C> is for the{" "}
            <DocLink href="/docs/run-a-node/#work-directory">
              native node
            </DocLink>
            . It outlives the container: <C>docker rm</C>,{" "}
            <C>docker compose down</C> and upgrades leave it in place.
          </p>
        </Prose>
        <Terms
          mono
          items={[
            {
              term: "<chain id>/frontier/",
              text: "The saved chain state, with its ledgers: a restart picks up from it. Delete it and the node syncs from the start.",
            },
            {
              term: "ledger-accounts/",
              text: "The accounts of the staking, next epoch and root ledgers. The node replaces them at each start.",
            },
            {
              term: "p2p-secret-key",
              text: "The node's peer identity. Keep it for a stable peer id; it is private, like a key.",
            },
            {
              term: ".mina/circuit-blobs/",
              text: "The verifier indexes, built at the first start. Safe to delete: the next start builds them again.",
            },
            {
              term: "proof-cache/",
              text: "The SNARK pool's cached proofs. Safe to delete while the node is stopped.",
            },
            {
              term: "mina.log, mina.log.1, ...",
              text: "The logs. Safe to delete.",
            },
            {
              term: "snark-worker-included.json",
              text: "The worker's work that blocks included.",
            },
            {
              term: "bug-reports/",
              text: "Reports mina report saved.",
            },
          ]}
        />
        <Prose>
          <p>
            To back it up, stop the node and copy the directory, for example
            with <C>tar -czf mina-state.tar.gz -C ~ mina-state</C>. Keep the
            copy private: it holds the peer identity. Never share a work
            directory between networks.
          </p>
          <p>
            A named volume works too: <C>-v mina-data:/data</C>, without{" "}
            <C>--user</C>. The node then runs as the image&apos;s own user (uid
            65532), and Docker keeps the files in its own storage.
          </p>
          <p>
            If the node stops at start with <C>Permission denied</C> at{" "}
            <C>/data</C>, the directory isn&apos;t writable by the user it runs
            as. Create it yourself before the first start and run the container
            with <C>--user &quot;$(id -u):$(id -g)&quot;</C>: if Docker creates
            a missing directory, root owns it.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="reuse" title="Reuse a native node's state">
        <Prose>
          <p>
            A container may take over the state of a node you ran with the
            release binary, keeping its chain and peer id. Stop the native node
            first: never run two nodes on one work directory at the same time.
            Run the container as yourself, so the files keep their owner and the
            native binary can use the directory again later.
          </p>
        </Prose>
        <CodeBlock
          code={reuse}
          label="move a native node's state into Docker"
        />
        <Prose>
          <p>
            In Compose, set <C>MINA_STATE_DIR</C> to the directory&apos;s full
            path, such as <C>/srv/mina-state</C>, and your <C>UID</C> and{" "}
            <C>GID</C> in <C>.env</C>.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="logs" title="Logs and bug reports">
        <Prose>
          <p>
            <C>docker logs -f mina</C> follows the log. The node also keeps its{" "}
            <DocLink href="/docs/run-a-node/#logs">rotated log files</DocLink>{" "}
            in the work directory, which a new container keeps and{" "}
            <C>mina report</C> reads. The compose file and the <C>--log-opt</C>{" "}
            options in the <C>docker run</C> commands above limit Docker&apos;s
            own copy of the log to 30 MiB.
          </p>
          <p>
            To collect a bug report, run <C>mina report</C> in the container.
            With <C>-it</C> it asks before printing the link to a pre-filled
            GitHub issue; without a terminal, add <C>--yes</C>. Or save it in
            the work directory with <C>--output</C>. Then follow{" "}
            <DocLink href="/docs/bug-reports/">reporting a bug</DocLink>.
          </p>
          <p>
            In the container, <C>~</C> is <C>/data</C>: a report saved at{" "}
            <C>~/bug-reports/</C> is in <C>~/mina-state/bug-reports/</C> on the
            host.
          </p>
        </Prose>
        <CodeBlock code={report} label="collect a bug report" />
      </DocSection>

      <DocSection id="stop-and-upgrade" title="Stop, start and upgrade">
        <Prose>
          <p>
            <C>docker stop mina</C> stops the node, and <C>docker start mina</C>{" "}
            starts it again from its saved chain. So does{" "}
            <C>docker compose up -d</C> when it recreates the container. With{" "}
            <C>--restart unless-stopped</C>, it also starts again after a reboot
            or a crash, unless you stopped it.
          </p>
          <p>
            To upgrade, pull the new tag, check its digest as in{" "}
            <DocLink href="/docs/docker/#check-the-image">
              check the image
            </DocLink>
            , and run a new container on the same work directory:
          </p>
        </Prose>
        <CodeBlock code={upgrade} label="upgrade with docker run" />
        <CodeBlock code={composeUpgrade} label="upgrade with Docker Compose" />
      </DocSection>

      <DocSection id="health" title="Health check">
        <Prose>
          <p>
            The image checks the node every minute with{" "}
            <C>mina node status --check</C>: healthy means synced, with a recent
            tip and at least one peer. The first 10 minutes don&apos;t count, to
            leave time to sync. <C>docker ps</C> shows the result. The check
            asks port 3000 inside the container, so change only the host side of
            the port.
          </p>
        </Prose>
        <CodeBlock
          code={`docker inspect --format '{{.State.Health.Status}}' mina`}
          label="health status"
        />
      </DocSection>

      <DocSection id="snark-worker" title="SNARK worker">
        <Prose>
          <p>
            The compose file starts the node with <C>--allow-worker-control</C>,
            so you start, stop and price the worker on the dashboard&apos;s
            SNARK worker page. To start the worker with the node instead, add{" "}
            <C>--run-snarker B62q... --snarker-fee FEE</C> to the command, or to{" "}
            <C>MINA_NODE_ARGS</C>. The worker needs no private key. Fees and
            choosing jobs are on the{" "}
            <DocLink href="/docs/snark-worker/">SNARK worker</DocLink> page.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="block-producer" title="Block producer">
        <Prose>
          <p>
            A producer adds <C>docker-compose.block-producer.yml</C> to the
            compose file. It mounts the key file read-only at{" "}
            <C>/keys/producer-key</C>, from <C>./keys/producer-key</C>, outside
            the work directory, and reads the key&apos;s password from{" "}
            <C>producer.env</C>. It also waits 3 s into each won slot before
            sending the block. See{" "}
            <DocLink href="/docs/block-producer/">block producer</DocLink> for
            the key and the producer page.
          </p>
          <p>
            Put an existing key file at <C>keys/producer-key</C>, readable by
            you only. Or make a new one in the container:
          </p>
        </Prose>
        <CodeBlock code={keygen} label="make a key file" />
        <Prose>
          <p>
            Save the password, readable by you only. <C>COMPOSE_FILE</C> in{" "}
            <C>.env</C> adds the producer file to every <C>docker compose</C>{" "}
            command. Check that the node can open the key: the{" "}
            <C>wallet address</C> command prints its public key. For rewards to
            another account, add{" "}
            <C>MINA_NODE_ARGS=--coinbase-receiver B62q...</C> to <C>.env</C>.
          </p>
        </Prose>
        <CodeBlock code={producerEnv} label="save the password" />
        <CodeBlock code={producerStart} label="check the key and start" />
        <Caution title="Keep the password out of commands and files you share">
          <p>
            Put it only in <C>producer.env</C>: not in the compose files, not in{" "}
            <C>.env</C>, and not on a command line, where other users and your
            shell history can see it. Never commit <C>producer.env</C> or the
            key. The file holds the password in single quotes, so choose one
            without a single quote. Anyone who can run Docker commands on the
            machine can read the container&apos;s environment.
          </p>
        </Caution>
        <CodeBlock
          code={composeProducer}
          label="docker-compose.block-producer.yml"
        />
      </DocSection>

      {/* Hidden until the web node launches, with the run-a-relay page. */}
      {showRelay ? (
        <DocSection id="relay" title="Relay">
          <Prose>
            <p>
              A relay is a node other nodes list as a peer, so its address and
              peer id have to stay the same. The peer id comes from{" "}
              <C>p2p-secret-key</C> in the work directory: keep the directory
              and the id survives restarts, upgrades and new containers. Give it
              your public address and open TCP port 8302.
            </p>
          </Prose>
          <CodeBlock code={relayStart} label="start a relay" />
          <Prose>
            <p>
              Relaying to browser nodes also needs the WebRTC signaling
              listener, UDP ports and usually an HTTPS proxy, set up on{" "}
              <DocLink href="/docs/run-a-relay/">run a relay</DocLink>. That
              page uses the release binary; its Docker setup isn&apos;t covered
              here yet.
            </p>
          </Prose>
          <CodeBlock code={composeRelay} label="docker-compose.relay.yml" />
        </DocSection>
      ) : null}

      <DocSection id="macos-and-windows" title="macOS and Windows">
        <Prose>
          <p>
            Docker Desktop runs the Linux image in a virtual machine. On a Mac
            with Apple silicon, Docker pulls the ARM64 image, which runs
            natively; don&apos;t force <C>--platform linux/amd64</C>. On an
            Intel Mac or a Windows PC, it runs the x86-64 image, so the CPU
            needs BMI2 and ADX, as on{" "}
            <DocLink href="/docs/requirements/">Linux</DocLink>.
          </p>
          <p>
            Give Docker Desktop&apos;s virtual machine the memory the{" "}
            <DocLink href="/docs/requirements/">requirements</DocLink> list for
            your role (Settings, Resources). On Windows, run the commands above
            in a WSL 2 terminal. The dashboard is at{" "}
            <C>http://127.0.0.1:3000</C>, as on Linux.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="build" title="Build the image yourself">
        <Prose>
          <p>
            The image is built from the release archives, not from source.{" "}
            <C>tools/release/docker/build-image.sh</C> builds the same image
            from the downloaded release, with Docker or Podman. It checks{" "}
            <C>SHA256SUMS.sig</C> against the repository&apos;s{" "}
            <C>docs/release/allowed_signers</C> and the archives against{" "}
            <C>SHA256SUMS</C>. Then it builds the image for this machine as{" "}
            <C>{image}</C>.
          </p>
        </Prose>
        <CodeBlock code={build} label="build the image from the release" />
      </DocSection>
    </DocsPage>
  );
}
