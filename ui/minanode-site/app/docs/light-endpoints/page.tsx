import type { Metadata } from "next";

import { CodeBlock } from "@minanode/design-system/patterns/code-block";

import { DocsPage } from "@/components/docs/docs-page";
import { C, DocLink, DocSection, Prose, Terms } from "@/components/docs/prose";
import { lightEndpoints } from "@/content/docs";

export const metadata: Metadata = {
  title: "Light client endpoints",
  description:
    "Read the best tip and account Merkle paths from a minanode node, with their limits and errors, and verify them.",
  alternates: { canonical: "/docs/light-endpoints/" },
};

// A devnet account (a block stake winner), used in every example.
const setAccount = `pk=B62qqLUmFPKkRhNUXY4N8yoyoDTWmkYz585AFYNK85cjfGyjYBLNvmz
node=http://127.0.0.1:3000`;

const bestTip = `curl -s $node/light/best-tip \\
  | jq '{state_hash, height, ledger_hashes, root: .root.state_hash}'`;

const bestTipBinprot = `curl -s -D - -o best-tip.bin "$node/light/best-tip?format=binprot"`;

const account = `curl -s "$node/light/account/$pk?ledger=staged" \\
  | jq '{ledger_hash, ledger_hash_field, block, index, leaf_hash, summary}'`;

const accountAtRoot = `root=$(curl -s $node/light/best-tip | jq -r .root.state_hash)
curl -s "$node/light/account/$pk?ledger=snarked&block=$root" \\
  | jq '{ledger_hash, ledger_hash_field, block, index}'`;

const refusal = `curl -s -w ' %{http_code}\\n' "$node/light/account/$pk?ledger=next"`;

const refusalAnswer = `{"code":"ledger_unavailable","error":"the node does not hold the next ledger jwqvkVGCsqoE3HFRux78jJTM1dTZfZntYK6yfobQAQSXCLjh8VQ"} 503`;

const graphqlAccount = `jq -n --arg pk $pk '{
  query: "query($pk: PublicKey!) { account(publicKey: $pk) { index leafHash merklePath { left right } } }",
  variables: {pk: $pk}
}' | curl -s $node/graphql -H 'content-type: application/json' -d @- \\
  | jq '.data.account | {index, leafHash, path: (.merklePath | length)}'`;

const graphqlMembership = `root=$(curl -s $node/light/best-tip | jq -r .root.state_hash)
jq -n --arg pk $pk --arg root $root '{
  query: "query($pk: PublicKey!, $root: String!) { encodedSnarkedLedgerAccountMembership(accountInfos: [{publicKey: $pk}], stateHash: $root) { account merklePath { left right } } }",
  variables: {pk: $pk, root: $root}
}' | curl -s $node/graphql -H 'content-type: application/json' -d @- \\
  | jq '.data.encodedSnarkedLedgerAccountMembership[0].merklePath | length'`;

const probe = `cargo run --release -p mina-light-probe -- \\
  --http $node --graphql $node/graphql --account $pk`;

export default function LightEndpointsPage() {
  return (
    <DocsPage
      slug="light-endpoints"
      title="Light client endpoints"
      lead="A light client checks the chain against a node it does not trust. The node serves the best tip's header, and an account with its Merkle path to a ledger hash in a block the client verifies."
    >
      <DocSection id="where" title="Where they are served">
        <Prose>
          <p>
            The endpoints are on the node&apos;s HTTP port, with the dashboard
            and API, and need no flag. They follow <C>--http-addr</C>, which is{" "}
            <C>127.0.0.1</C> by default. Before you open the port to other
            machines, read{" "}
            <DocLink href="/docs/dashboard/#on-a-server">
              viewing it on a server
            </DocLink>
            .
          </p>
          <p>
            A node started with <C>--public-graphql</C> answers 404 for{" "}
            <C>/light/*</C>, and its public GraphQL refuses the path fields
            below.
          </p>
          <p>
            The examples use <C>curl</C> and <C>jq</C>, and these two shell
            variables: a devnet account and the node&apos;s address.
          </p>
        </Prose>
        <CodeBlock code={setAccount} label="set the account and node" />
      </DocSection>

      <DocSection id="best-tip" title="GET /light/best-tip">
        <Prose>
          <p>
            The best tip as a light client verifies it: the tip&apos;s header
            (its protocol state and proof), the root&apos;s protocol state, and
            the state body hashes that link the root to the tip. It carries no
            block bodies. The root is the oldest block the node keeps, 290
            blocks behind the tip.
          </p>
          <p>
            <C>format=json</C> is the default. <C>format=binprot</C> returns the
            three values back to back, in OCaml&apos;s types{" "}
            <C>Mina_block.Header.t</C>, <C>Mina_state.Protocol_state.Value.t</C>{" "}
            and <C>State_body_hash.t list</C>, as{" "}
            <C>application/octet-stream</C>. Both forms set the{" "}
            <C>x-mina-state-hash</C> header to the tip&apos;s state hash.
          </p>
          <p data-claim={lightEndpoints.bestTipSize.claim}>
            {lightEndpoints.bestTipSize.text}
          </p>
        </Prose>
        <Terms
          mono
          items={[
            {
              term: "state_hash, height",
              text: "The tip.",
            },
            {
              term: "genesis_state_hash",
              text: "The genesis state hash the tip carries. Compare it with the one you pin.",
            },
            {
              term: "global_slot_since_genesis",
              text: "The tip's slot.",
            },
            {
              term: "ledger_hashes",
              text: "The tip's staged, snarked, staking and next ledger hashes.",
            },
            {
              term: "header",
              text: "Base64 of the tip header's binprot: protocol state, proof and protocol versions.",
            },
            {
              term: "root",
              text: "The root's state_hash, height and protocol_state (base64 binprot).",
            },
            {
              term: "body_hashes, body_hash_count",
              text: "Base64 of the binprot list of state body hashes, from the root's child to the tip.",
            },
            {
              term: "trust",
              text: "What a client may rely on, as a sentence.",
            },
          ]}
        />
        <Prose>
          <p>
            Every field but <C>header</C>, <C>root.protocol_state</C> and{" "}
            <C>body_hashes</C> is a convenience copy. Take the values from those
            three after you verify them.
          </p>
        </Prose>
        <CodeBlock code={bestTip} label="the best tip, summarised with jq" />
        <CodeBlock code={bestTipBinprot} label="the binprot form" />
      </DocSection>

      <DocSection id="account" title="GET /light/account/{public_key}">
        <Prose>
          <p>
            One account in a named ledger of a best-chain block, with its index,
            leaf hash and Merkle path. The answer names the ledger hash the path
            folds to, where that hash sits in the block&apos;s protocol state,
            and the block.
          </p>
        </Prose>
        <Terms
          mono
          items={[
            {
              term: "ledger",
              text: (
                <>
                  Required: <C>staged</C>, <C>snarked</C>, <C>staking</C> or{" "}
                  <C>next</C>. See{" "}
                  <DocLink href="#ledgers">which ledgers a node holds</DocLink>.
                </>
              ),
            },
            {
              term: "token",
              text: "A token id. MINA if absent.",
            },
            {
              term: "block",
              text: "A best-chain block's state hash. The best tip if absent.",
            },
          ]}
        />
        <Prose>
          <p>The answer has these fields:</p>
        </Prose>
        <Terms
          mono
          items={[
            {
              term: "ledger_hash",
              text: "The root the Merkle path folds to.",
            },
            {
              term: "ledger_hash_field",
              text: "Where ledger_hash sits in the block's protocol state.",
            },
            {
              term: "block",
              text: "The state_hash, height and global_slot_since_genesis of the block whose ledger was read.",
            },
            {
              term: "public_key, token_id, index",
              text: "The account and its leaf position, left to right.",
            },
            {
              term: "leaf_hash",
              text: "The account's hash, as a decimal field element.",
            },
            {
              term: "merkle_path",
              text: "35 elements, leaf first. left set means the node is a left child and left holds its sibling's hash; right means the reverse.",
            },
            {
              term: "account",
              text: "Base64 of the account's binprot. Hash it to get leaf_hash.",
            },
            {
              term: "summary",
              text: "Balance in nanomina, nonce, delegate and whether it has a zkApp: convenience copies.",
            },
            {
              term: "trust",
              text: "What a client may rely on for this ledger.",
            },
          ]}
        />
        <Prose>
          <p data-claim={lightEndpoints.accountSize.claim}>
            {lightEndpoints.accountSize.text}
          </p>
        </Prose>
        <CodeBlock
          code={account}
          label="an account in the tip's staged ledger"
        />
        <CodeBlock
          code={accountAtRoot}
          label="an account in the root's snarked ledger"
        />
      </DocSection>

      <DocSection id="ledgers" title="Which ledgers a node holds">
        <Terms
          mono
          items={[
            {
              term: "staged",
              text: (
                <>
                  Every best-chain block. The hash is at{" "}
                  <C>
                    blockchain_state.staged_ledger_hash.non_snark.ledger_hash
                  </C>
                  . The block producer commits to it, and the block proof does
                  not check it.
                </>
              ),
            },
            {
              term: "snarked",
              text: (
                <>
                  The root only. At the tip the answer is{" "}
                  <C>ledger_unavailable</C> with a <C>hint</C>: ask again with{" "}
                  <C>block</C> set to <C>root.state_hash</C> from{" "}
                  <C>/light/best-tip</C>. The hash is at{" "}
                  <C>
                    blockchain_state.ledger_proof_statement.target.first_pass_ledger
                  </C>
                  .
                </>
              ),
            },
            {
              term: "staking",
              text: (
                <>
                  The tip&apos;s staking epoch ledger. The hash is at{" "}
                  <C>consensus_state.staking_epoch_data.ledger.hash</C>.
                </>
              ),
            },
            {
              term: "next",
              text: (
                <>
                  The tip&apos;s next epoch ledger, once the root is the last
                  block before the tip&apos;s epoch. The hash is at{" "}
                  <C>consensus_state.next_epoch_data.ledger.hash</C>.
                </>
              ),
            },
          ]}
        />
        <Prose>
          <p>
            The next ledger is unavailable for about 290 blocks after each epoch
            boundary. At its first block of an epoch, the tip takes the snarked
            ledger of the previous epoch&apos;s last block as its next ledger.
            The node holds that ledger once that block is the root, 290 blocks
            later. Until then, <C>ledger=next</C> answers{" "}
            <C>ledger_unavailable</C>.
          </p>
          <p data-claim={lightEndpoints.nextWindow.claim}>
            {lightEndpoints.nextWindow.text}
          </p>
          <p>
            Every new block moves the root. Take the root from a fresh{" "}
            <C>/light/best-tip</C>: a root one block old may already answer{" "}
            <C>block_not_in_best_chain</C>.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="errors" title="Errors">
        <Prose>
          <p>
            An error is JSON with a readable <C>error</C> and a stable{" "}
            <C>code</C>. Match on <C>code</C>.
          </p>
        </Prose>
        <Terms
          mono
          items={[
            {
              term: "bad_request",
              text: "400. A bad key, token, block hash, ledger or format, or no ledger. The request does not reach the node.",
            },
            {
              term: "not_synced",
              text: "503. The node has no best tip, or is bootstrapping. Catch-up of new blocks does not count.",
            },
            {
              term: "block_not_in_best_chain",
              text: "404. The block given is not in the best chain, for example because the root moved past it.",
            },
            {
              term: "account_not_found",
              text: "404. No such account in that ledger. A Merkle path cannot prove absence, so this is the node's word only.",
            },
            {
              term: "ledger_unavailable",
              text: "503. The node does not hold that ledger. For snarked, a hint names the ledger it holds.",
            },
            {
              term: "busy",
              text: "503. Too many light requests or accounts are pending. Retry later.",
            },
            {
              term: "timeout",
              text: "503. The node did not answer in time.",
            },
            {
              term: "internal",
              text: "500. For example, a path that does not fold to its hash. The node checks each path before it answers.",
            },
          ]}
        />
        <CodeBlock code={refusal} label="ask for the next ledger" />
        <CodeBlock
          code={refusalAnswer}
          label="answer while the next ledger is unavailable"
        />
      </DocSection>

      <DocSection id="limits" title="Limits">
        <Terms
          items={lightEndpoints.limits.map(limit => ({
            term: limit.label,
            text: <span data-claim={limit.claim}>{limit.value}</span>,
          }))}
        />
        <Prose>
          <p>
            The best tip is encoded once per tip and root. Concurrent requests
            wait for that encoding instead of starting their own.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="graphql" title="GraphQL">
        <Prose>
          <p>
            Two GraphQL fields give the same paths, in OCaml&apos;s names and
            shapes, at <C>/graphql</C> on the HTTP port. Both are under the same
            limits as <C>/light/*</C>. When those are full, they answer the
            GraphQL error <C>too many light requests in flight</C>.
          </p>
          <ul>
            <li>
              <C>
                account(publicKey, token) &#123; index leafHash merklePath
                &#125;
              </C>
              : the path in the best tip&apos;s staged ledger. All three come
              from one read of one ledger snapshot. They are null while the node
              bootstraps.
            </li>
            <li>
              <C>
                encodedSnarkedLedgerAccountMembership(accountInfos, stateHash)
              </C>
              : each account as base64 binprot, with its path in the snarked
              ledger of the best-chain block <C>stateHash</C>. Use the
              root&apos;s state hash. OCaml has no cap on <C>accountInfos</C>;
              this node takes at most 32.
            </li>
          </ul>
        </Prose>
        <CodeBlock code={graphqlAccount} label="GraphQL account path" />
        <CodeBlock
          code={graphqlMembership}
          label="GraphQL snarked ledger membership at the root"
        />
      </DocSection>

      <DocSection id="verify" title="Verify what you get">
        <Prose>
          <p>
            Every value comes from one node, so check it before you rely on it.
          </p>
          <ol>
            <li>
              Decode <C>header</C>. Verify its proof against devnet&apos;s
              blockchain verification key, and check that its genesis state hash
              is the one you pin.
            </li>
            <li>
              For an answer about the root, link the root to the tip. Start from
              the hash of <C>root.protocol_state</C> and fold each body hash in
              order as <C>state_hash = H(previous state_hash, body_hash)</C>.
              The last result must be the tip&apos;s state hash.
            </li>
            <li>
              Hash the decoded <C>account</C>: the result must be{" "}
              <C>leaf_hash</C>. Fold <C>merkle_path</C> from the leaf with
              Mina&apos;s Merkle hash for each height. The result must be{" "}
              <C>ledger_hash</C>.
            </li>
            <li>
              Check that <C>block.state_hash</C> is the tip or root you
              verified, and that <C>ledger_hash</C> is the value at{" "}
              <C>ledger_hash_field</C> in that block&apos;s protocol state.
            </li>
          </ol>
          <p>
            If any check fails, discard the answer. A node that lies cannot make
            a path verify: it can only refuse, or answer for another block,
            which the last check catches.
          </p>
          <p>
            Two limits remain. <C>account_not_found</C> is not provable. One
            node&apos;s best tip is not proof of the best chain: ask several
            nodes, compare their headers with Mina&apos;s fork choice rule, and
            check the tip&apos;s slot against the clock.
          </p>
          <p>
            The repository&apos;s <C>tools/light-probe</C> runs these checks
            against a node. Run it in a checkout of the repository: it reads
            devnet&apos;s verification key from the checkout&apos;s{" "}
            <C>data/vk/</C>.
          </p>
        </Prose>
        <CodeBlock code={probe} label="verify a node's answers" />
        <Prose>
          <p>
            It verifies the tip, the body hashes, the account in each ledger
            (the snarked one at the root), and the GraphQL staged path. It
            reports a ledger the node does not hold as not verified.
          </p>
        </Prose>
      </DocSection>
    </DocsPage>
  );
}
